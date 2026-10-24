//! Denoising.
//!
//! **OIDN** — Intel Open Image Denoise 2.x is `dlopen`ed at runtime (no
//! build-time linking, so the crate builds anywhere). Search order:
//! `$ARGENT_OIDN_DIR/lib`, then `~/.local/opt/oidn*/lib` (newest version
//! first), then the dynamic loader's own search (`LD_LIBRARY_PATH`, system).
//! The release tarball's RPATH (`$ORIGIN/../lib`) resolves its bundled TBB.
//! Only a **CPU** device is ever created (`OIDN_DEVICE_TYPE_CPU`; OIDN ≥ 2.3
//! loads device modules lazily, so the HIP/CUDA/SYCL modules — and the GPUs —
//! are never touched). The device is created once and cached; filters are
//! kept per image size and option set. Each call uses the RT filter with
//! `hdr = true` and `quality = high` by default; when albedo (and normal) are
//! given they are first prefiltered in place with their own RT filters and
//! the beauty pass then runs with `cleanAux = true`, as the OIDN manual
//! recommends for final frames. [`denoise_with`] can skip the prefilter
//! (for noise-free first-hit guides) and lower the quality mode. OIDN
//! cannot use a normal without an albedo, so a lone normal is ignored. Thread count follows OIDN's `OIDN_NUM_THREADS` env var (default:
//! all cores); thread pinning is disabled so it shares the machine politely.
//!
//! **À-trous** — the built-in fallback: an edge-avoiding à-trous wavelet
//! filter (Dammertz et al. 2010) with SVGF-style luminance edge stopping
//! (Schied et al. 2017): 5 passes of the 5×5 B3-spline kernel with holes
//! 1, 2, 4, 8, 16 px, weights from the luminance difference relative to a
//! per-pixel noise estimate, the normal (`max(0, n·n')^128`) and the albedo
//! (`exp(−|Δa|²/0.01)`). With albedo the image is demodulated first, so
//! texture detail is not blurred. Rayon-parallel; ~0.1–0.2 s at 1080×1920.
//!
//! Non-finite or negative radiance is treated as 0 by both backends.

use crate::Image;
use rayon::prelude::*;
use std::path::PathBuf;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DenoiseBackend {
    /// OIDN when it loads, else à-trous.
    Auto,
    Oidn,
    Atrous,
    None,
}

impl DenoiseBackend {
    /// The backend [`denoise`] will actually run (`Auto` resolved).
    pub fn resolved(self) -> DenoiseBackend {
        match self {
            DenoiseBackend::Auto if oidn_available() => DenoiseBackend::Oidn,
            DenoiseBackend::Auto => DenoiseBackend::Atrous,
            other => other,
        }
    }
}

pub struct DenoiseInput<'a> {
    pub color: &'a Image,
    /// First-hit albedo in 0..1 (same size), improves edge preservation.
    pub albedo: Option<&'a Image>,
    /// First-hit shading normal in −1..1 (same size).
    pub normal: Option<&'a Image>,
}

/// True when the OIDN library loads and its CPU device commits (cached).
pub fn oidn_available() -> bool {
    oidn::device().is_ok()
}

/// OIDN version reported by the device, e.g. `"2.5.1"`.
pub fn oidn_version() -> Option<String> {
    oidn::device().ok().map(|d| d.version.clone())
}

/// Path of the OIDN library that was loaded.
pub fn oidn_library() -> Option<PathBuf> {
    oidn::device().ok().map(|d| d.path.clone())
}

/// Why OIDN is unavailable (`None` when it loaded).
pub fn oidn_error() -> Option<String> {
    oidn::device().err().map(str::to_owned)
}

/// OIDN quality mode (`quality` filter parameter).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum DenoiseQuality {
    /// Final frames (OIDN_QUALITY_HIGH).
    #[default]
    High,
    /// Interactive/real-time quality, faster (OIDN_QUALITY_BALANCED).
    Balanced,
    /// Previews (OIDN_QUALITY_FAST; OIDN uses balanced for prefiltering).
    Fast,
}

/// Knobs for [`denoise_with`]; the defaults are what [`denoise`] uses.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DenoiseOptions {
    /// Denoise albedo/normal with their own RT filters before the beauty
    /// pass (OIDN's recommendation for path-traced, noisy aux buffers).
    /// Set `false` when the guides are already noise-free (e.g. deterministic
    /// first-hit buffers with only AA/DOF jitter): they are then passed
    /// straight to the beauty filter, still with `cleanAux = true`, which
    /// saves the two prefilter passes. Default `true`.
    pub prefilter_aux: bool,
    /// Default [`DenoiseQuality::High`].
    pub quality: DenoiseQuality,
}

impl Default for DenoiseOptions {
    fn default() -> Self {
        DenoiseOptions { prefilter_aux: true, quality: DenoiseQuality::High }
    }
}

pub fn denoise(input: DenoiseInput, backend: DenoiseBackend) -> anyhow::Result<Image> {
    denoise_with(input, backend, &DenoiseOptions::default())
}

/// [`denoise`] with explicit options (they only affect OIDN; à-trous ignores them).
pub fn denoise_with(input: DenoiseInput, backend: DenoiseBackend, opts: &DenoiseOptions) -> anyhow::Result<Image> {
    if backend == DenoiseBackend::None {
        return Ok(input.color.clone());
    }
    validate(&input)?;
    match backend {
        DenoiseBackend::Atrous => Ok(atrous(&input)),
        DenoiseBackend::Oidn => oidn_denoise(&input, opts),
        DenoiseBackend::Auto => {
            if !oidn_available() {
                return Ok(atrous(&input));
            }
            match oidn_denoise(&input, opts) {
                Ok(img) => Ok(img),
                Err(e) => {
                    eprintln!("argent-post: OIDN failed ({e:#}); falling back to à-trous");
                    Ok(atrous(&input))
                }
            }
        }
        DenoiseBackend::None => unreachable!(),
    }
}

fn validate(input: &DenoiseInput) -> anyhow::Result<()> {
    let c = input.color;
    anyhow::ensure!(
        c.pixels.len() == c.width as usize * c.height as usize,
        "color image has {} pixels for {}x{}",
        c.pixels.len(),
        c.width,
        c.height
    );
    for (name, aux) in [("albedo", input.albedo), ("normal", input.normal)] {
        if let Some(a) = aux {
            anyhow::ensure!(
                a.width == c.width && a.height == c.height && a.pixels.len() == c.pixels.len(),
                "{name} is {}x{} but color is {}x{}",
                a.width,
                a.height,
                c.width,
                c.height
            );
        }
    }
    Ok(())
}

#[inline]
fn clean_radiance(v: f32) -> f32 {
    if v.is_finite() { v.max(0.0) } else { 0.0 }
}

#[inline]
fn clean_unit(v: f32, lo: f32) -> f32 {
    if v.is_finite() { v.clamp(lo, 1.0) } else { 0.0 }
}

fn oidn_denoise(input: &DenoiseInput, opts: &DenoiseOptions) -> anyhow::Result<Image> {
    let dev = oidn::device().map_err(|e| anyhow::anyhow!("OIDN not available: {e}"))?;
    let (w, h) = (input.color.width as usize, input.color.height as usize);
    if w == 0 || h == 0 {
        return Ok(input.color.clone());
    }
    let mut color: Vec<[f32; 3]> = input.color.pixels.par_iter().map(|p| p.map(clean_radiance)).collect();
    let mut albedo: Option<Vec<[f32; 3]>> =
        input.albedo.map(|a| a.pixels.par_iter().map(|p| p.map(|v| clean_unit(v, 0.0))).collect());
    let mut normal: Option<Vec<[f32; 3]>> = match (&albedo, input.normal) {
        (Some(_), Some(n)) => Some(n.pixels.par_iter().map(|p| p.map(|v| clean_unit(v, -1.0))).collect()),
        _ => None,
    };
    let mut out = vec![[0.0f32; 3]; w * h];
    let quality = match opts.quality {
        DenoiseQuality::High => oidn::QUALITY_HIGH,
        DenoiseQuality::Balanced => oidn::QUALITY_BALANCED,
        DenoiseQuality::Fast => oidn::QUALITY_FAST,
    };
    dev.run(&mut color, albedo.as_deref_mut(), normal.as_deref_mut(), &mut out, w, h, opts.prefilter_aux, quality)
        .map_err(|e| anyhow::anyhow!("OIDN: {e}"))?;
    Ok(Image { width: input.color.width, height: input.color.height, pixels: out })
}

mod oidn {
    use libloading::Library;
    use std::ffi::{CStr, c_char, c_int, c_void};
    use std::path::{Path, PathBuf};
    use std::sync::{Mutex, OnceLock};

    type Dev = *mut c_void;
    type Filt = *mut c_void;

    // Values from include/OpenImageDenoise/oidn.h (2.5.1).
    const DEVICE_TYPE_CPU: c_int = 1;
    const ERROR_NONE: c_int = 0;
    const FORMAT_FLOAT3: c_int = 3;
    pub(super) const QUALITY_FAST: c_int = 4;
    pub(super) const QUALITY_BALANCED: c_int = 5;
    pub(super) const QUALITY_HIGH: c_int = 6;

    struct Api {
        new_device: unsafe extern "C" fn(c_int) -> Dev,
        commit_device: unsafe extern "C" fn(Dev),
        set_device_bool: unsafe extern "C" fn(Dev, *const c_char, bool),
        get_device_int: unsafe extern "C" fn(Dev, *const c_char) -> c_int,
        get_device_error: unsafe extern "C" fn(Dev, *mut *const c_char) -> c_int,
        new_filter: unsafe extern "C" fn(Dev, *const c_char) -> Filt,
        release_filter: unsafe extern "C" fn(Filt),
        #[allow(clippy::type_complexity)]
        set_shared_filter_image:
            unsafe extern "C" fn(Filt, *const c_char, *mut c_void, c_int, usize, usize, usize, usize, usize),
        set_filter_bool: unsafe extern "C" fn(Filt, *const c_char, bool),
        set_filter_int: unsafe extern "C" fn(Filt, *const c_char, c_int),
        commit_filter: unsafe extern "C" fn(Filt),
        execute_filter: unsafe extern "C" fn(Filt),
        /// Keeps the symbols valid; never unloaded.
        _lib: Library,
    }

    /// (width, height, albedo, normal, prefilter aux, quality).
    type Key = (usize, usize, bool, bool, bool, c_int);
    const FILTER_CACHE: usize = 4;

    /// Filters cached for one image size, aux configuration and option set.
    struct Filters {
        key: Key,
        beauty: Filt,
        albedo: Option<Filt>,
        normal: Option<Filt>,
    }

    pub(super) struct Device {
        api: Api,
        dev: Dev,
        pub version: String,
        pub path: PathBuf,
        /// Recently used filter sets, most recent last (filters on one device
        /// share their scratch memory, so a few cost little).
        filters: Mutex<Vec<Filters>>,
    }

    // SAFETY: OIDN devices and filters may be used from any thread; all use
    // of the filters is serialised through `filters`.
    unsafe impl Send for Device {}
    unsafe impl Sync for Device {}

    pub(super) fn device() -> Result<&'static Device, &'static str> {
        static DEVICE: OnceLock<Result<Device, String>> = OnceLock::new();
        DEVICE.get_or_init(load).as_ref().map_err(String::as_str)
    }

    fn version_key(p: &Path) -> Vec<u32> {
        let name = p.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        name.split(|c: char| !c.is_ascii_digit()).filter(|s| !s.is_empty()).filter_map(|s| s.parse().ok()).collect()
    }

    fn candidates() -> Vec<PathBuf> {
        const NAMES: [&str; 2] = ["libOpenImageDenoise.so.2", "libOpenImageDenoise.so"];
        let mut dirs = Vec::new();
        if let Some(d) = std::env::var_os("ARGENT_OIDN_DIR") {
            let d = PathBuf::from(d);
            dirs.push(d.join("lib"));
            dirs.push(d.join("lib64"));
            dirs.push(d);
        }
        if let Some(home) = std::env::var_os("HOME") {
            let opt = PathBuf::from(home).join(".local/opt");
            if let Ok(rd) = std::fs::read_dir(&opt) {
                let mut found: Vec<PathBuf> = rd
                    .filter_map(Result::ok)
                    .filter(|e| e.file_name().to_string_lossy().starts_with("oidn"))
                    .map(|e| e.path())
                    .collect();
                found.sort_by_key(|p| std::cmp::Reverse(version_key(p)));
                dirs.extend(found.into_iter().map(|d| d.join("lib")));
            }
        }
        let mut out: Vec<PathBuf> =
            dirs.iter().flat_map(|d| NAMES.iter().map(move |n| d.join(n))).filter(|p| p.exists()).collect();
        // Bare names: the dynamic loader searches LD_LIBRARY_PATH and the system.
        out.extend(NAMES.iter().map(PathBuf::from));
        out
    }

    fn load() -> Result<Device, String> {
        let mut errors = Vec::new();
        for cand in candidates() {
            // SAFETY: loading OIDN runs its (trusted) library initialisers.
            match unsafe { Library::new(&cand) } {
                Ok(lib) => match unsafe { Device::create(lib, cand.clone()) } {
                    Ok(dev) => return Ok(dev),
                    Err(e) => errors.push(format!("{}: {e}", cand.display())),
                },
                Err(e) => errors.push(format!("{}: {e}", cand.display())),
            }
        }
        Err(format!(
            "libOpenImageDenoise not usable (set ARGENT_OIDN_DIR or unpack a release into ~/.local/opt/oidn-<ver>/): {}",
            errors.join("; ")
        ))
    }

    unsafe fn sym<T: Copy>(lib: &Library, name: &str) -> Result<T, String> {
        // SAFETY: the caller names the C signature from oidn.h.
        unsafe { lib.get::<T>(name).map(|s| *s).map_err(|e| format!("missing symbol {name}: {e}")) }
    }

    impl Api {
        fn error(&self, dev: Dev) -> Option<String> {
            let mut msg: *const c_char = std::ptr::null();
            // SAFETY: valid (or null) device, out-pointer to a local.
            let code = unsafe { (self.get_device_error)(dev, &mut msg) };
            if code == ERROR_NONE {
                return None;
            }
            let text = if msg.is_null() {
                String::new()
            } else {
                // SAFETY: OIDN returns a NUL-terminated string owned by the device.
                format!(": {}", unsafe { CStr::from_ptr(msg) }.to_string_lossy())
            };
            Some(format!("error {code}{text}"))
        }
    }

    impl Device {
        unsafe fn create(lib: Library, path: PathBuf) -> Result<Device, String> {
            // SAFETY (all): signatures transcribed from oidn.h 2.5.1.
            let api = unsafe {
                let new_device = sym(&lib, "oidnNewDevice")?;
                let commit_device = sym(&lib, "oidnCommitDevice")?;
                let set_device_bool = sym(&lib, "oidnSetDeviceBool")?;
                let get_device_int = sym(&lib, "oidnGetDeviceInt")?;
                let get_device_error = sym(&lib, "oidnGetDeviceError")?;
                let new_filter = sym(&lib, "oidnNewFilter")?;
                let release_filter = sym(&lib, "oidnReleaseFilter")?;
                let set_shared_filter_image = sym(&lib, "oidnSetSharedFilterImage")?;
                let set_filter_bool = sym(&lib, "oidnSetFilterBool")?;
                let set_filter_int = sym(&lib, "oidnSetFilterInt")?;
                let commit_filter = sym(&lib, "oidnCommitFilter")?;
                let execute_filter = sym(&lib, "oidnExecuteFilter")?;
                Api {
                    new_device,
                    commit_device,
                    set_device_bool,
                    get_device_int,
                    get_device_error,
                    new_filter,
                    release_filter,
                    set_shared_filter_image,
                    set_filter_bool,
                    set_filter_int,
                    commit_filter,
                    execute_filter,
                    _lib: lib,
                }
            };
            // SAFETY: plain C calls on the device we create here.
            unsafe {
                let dev = (api.new_device)(DEVICE_TYPE_CPU);
                if dev.is_null() {
                    let why = api.error(std::ptr::null_mut()).unwrap_or_default();
                    return Err(format!("oidnNewDevice(CPU) failed {why}"));
                }
                (api.set_device_bool)(dev, c"setAffinity".as_ptr(), false);
                (api.commit_device)(dev);
                if let Some(e) = api.error(dev) {
                    return Err(format!("CPU device commit: {e}"));
                }
                let v = (api.get_device_int)(dev, c"version".as_ptr());
                let version = format!("{}.{}.{}", v / 10000, (v / 100) % 100, v % 100);
                Ok(Device { api, dev, version, path, filters: Mutex::new(Vec::new()) })
            }
        }

        fn check(&self, what: &str) -> Result<(), String> {
            match self.api.error(self.dev) {
                None => Ok(()),
                Some(e) => Err(format!("{what}: {e}")),
            }
        }

        fn new_rt(&self, quality: c_int) -> Result<Filt, String> {
            // SAFETY: committed device; "RT" is a built-in filter type.
            let f = unsafe { (self.api.new_filter)(self.dev, c"RT".as_ptr()) };
            if f.is_null() {
                self.check("oidnNewFilter")?;
                return Err("oidnNewFilter returned null".into());
            }
            // SAFETY: valid filter handle.
            unsafe { (self.api.set_filter_int)(f, c"quality".as_ptr(), quality) };
            Ok(f)
        }

        fn create_filters(&self, key: Key) -> Result<Filters, String> {
            let (_, _, has_albedo, has_normal, prefilter, quality) = key;
            let beauty = self.new_rt(quality)?;
            // SAFETY: valid filter handle. Aux is clean either because it is
            // prefiltered or because the caller says it is noise-free.
            unsafe {
                (self.api.set_filter_bool)(beauty, c"hdr".as_ptr(), true);
                (self.api.set_filter_bool)(beauty, c"cleanAux".as_ptr(), has_albedo);
            }
            let mut f = Filters { key, beauty, albedo: None, normal: None };
            for (wanted, slot) in [(has_albedo, 0), (has_normal, 1)] {
                if wanted && prefilter {
                    match self.new_rt(quality) {
                        Ok(x) if slot == 0 => f.albedo = Some(x),
                        Ok(x) => f.normal = Some(x),
                        Err(e) => {
                            self.release(f);
                            return Err(e);
                        }
                    }
                }
            }
            Ok(f)
        }

        fn release(&self, filters: Filters) {
            // SAFETY: handles created by `new_rt`, released exactly once.
            unsafe {
                (self.api.release_filter)(filters.beauty);
                for f in [filters.albedo, filters.normal].into_iter().flatten() {
                    (self.api.release_filter)(f);
                }
            }
        }

        /// Binds `buf` (w×h FLOAT3, tightly packed) as image `name` of `filter`.
        unsafe fn bind(&self, filter: Filt, name: &CStr, buf: *mut [f32; 3], w: usize, h: usize) {
            // SAFETY: caller keeps `buf` alive and sized w*h until execution ends.
            unsafe {
                (self.api.set_shared_filter_image)(filter, name.as_ptr(), buf.cast(), FORMAT_FLOAT3, w, h, 0, 12, 12 * w)
            }
        }

        unsafe fn execute(&self, filter: Filt, what: &str) -> Result<(), String> {
            // SAFETY: images bound by the caller.
            unsafe {
                (self.api.commit_filter)(filter);
                self.check(&format!("{what} commit"))?;
                (self.api.execute_filter)(filter);
            }
            self.check(&format!("{what} execute"))
        }

        /// Optionally prefilters `albedo`/`normal` in place, then denoises
        /// `color` into `out`. The device is shared by all calls; filters are
        /// reused while size, aux set and options stay the same.
        #[allow(clippy::too_many_arguments)]
        pub(super) fn run(
            &self,
            color: &mut [[f32; 3]],
            albedo: Option<&mut [[f32; 3]]>,
            normal: Option<&mut [[f32; 3]]>,
            out: &mut [[f32; 3]],
            w: usize,
            h: usize,
            prefilter: bool,
            quality: c_int,
        ) -> Result<(), String> {
            let n = w * h;
            assert!(color.len() == n && out.len() == n);
            let key: Key = (w, h, albedo.is_some(), normal.is_some(), prefilter, quality);
            let mut cache = self.filters.lock().unwrap_or_else(|p| p.into_inner());
            let entry = match cache.iter().position(|f| f.key == key) {
                Some(i) => cache.remove(i),
                None => {
                    let f = self.create_filters(key)?;
                    if cache.len() >= FILTER_CACHE {
                        let old = cache.remove(0);
                        self.release(old);
                    }
                    f
                }
            };
            cache.push(entry);
            let f = cache.last().expect("just pushed");
            // SAFETY: every bound buffer is a live, exclusively borrowed w*h slice
            // that outlives the synchronous oidnExecuteFilter calls below.
            unsafe {
                let a_ptr = albedo.map(|a| {
                    assert_eq!(a.len(), n);
                    a.as_mut_ptr()
                });
                let n_ptr = normal.map(|v| {
                    assert_eq!(v.len(), n);
                    v.as_mut_ptr()
                });
                if let (Some(af), Some(ap)) = (f.albedo, a_ptr) {
                    self.bind(af, c"albedo", ap, w, h);
                    self.bind(af, c"output", ap, w, h);
                    self.execute(af, "albedo prefilter")?;
                }
                if let (Some(nf), Some(np)) = (f.normal, n_ptr) {
                    self.bind(nf, c"normal", np, w, h);
                    self.bind(nf, c"output", np, w, h);
                    self.execute(nf, "normal prefilter")?;
                }
                self.bind(f.beauty, c"color", color.as_mut_ptr(), w, h);
                if let Some(ap) = a_ptr {
                    self.bind(f.beauty, c"albedo", ap, w, h);
                }
                if let Some(np) = n_ptr {
                    self.bind(f.beauty, c"normal", np, w, h);
                }
                self.bind(f.beauty, c"output", out.as_mut_ptr(), w, h);
                self.execute(f.beauty, "beauty")
            }
        }
    }
}

// ---------------------------------------------------------------------------
// À-trous fallback.

const ATROUS_PASSES: u32 = 5;
const SIGMA_LUM: f32 = 4.0;
const NORMAL_POWER: i32 = 128;
const INV_SIGMA_ALBEDO2: f32 = 1.0 / 0.01;
/// Albedo floor for demodulation (black albedo would blow noise up).
const DEMOD_FLOOR: f32 = 0.02;
const B3: [f32; 5] = [1.0 / 16.0, 1.0 / 4.0, 3.0 / 8.0, 1.0 / 4.0, 1.0 / 16.0];

#[inline]
fn lum(p: [f32; 3]) -> f32 {
    0.2126 * p[0] + 0.7152 * p[1] + 0.0722 * p[2]
}

/// Separable blur with a symmetric kernel; borders renormalise.
fn blur_sep(src: &[f32], w: usize, h: usize, k: &[f32]) -> Vec<f32> {
    let r = (k.len() / 2) as isize;
    let mut tmp = vec![0.0f32; w * h];
    tmp.par_chunks_mut(w).enumerate().for_each(|(y, row)| {
        let s = &src[y * w..(y + 1) * w];
        for (x, o) in row.iter_mut().enumerate() {
            let (mut acc, mut ws) = (0.0, 0.0);
            for (i, kv) in k.iter().enumerate() {
                let xx = x as isize + i as isize - r;
                if xx >= 0 && (xx as usize) < w {
                    acc += kv * s[xx as usize];
                    ws += kv;
                }
            }
            *o = acc / ws;
        }
    });
    let mut out = vec![0.0f32; w * h];
    out.par_chunks_mut(w).enumerate().for_each(|(y, row)| {
        for (x, o) in row.iter_mut().enumerate() {
            let (mut acc, mut ws) = (0.0, 0.0);
            for (i, kv) in k.iter().enumerate() {
                let yy = y as isize + i as isize - r;
                if yy >= 0 && (yy as usize) < h {
                    acc += kv * tmp[yy as usize * w + x];
                    ws += kv;
                }
            }
            *o = acc / ws;
        }
    });
    out
}

/// Per-pixel noise variance of luminance: residual against the 3×3 mean
/// (×9/8 for white noise), averaged over 5×5.
fn noise_variance(l: &[f32], w: usize, h: usize) -> Vec<f32> {
    let mean = blur_sep(l, w, h, &[1.0, 1.0, 1.0]);
    let resid: Vec<f32> = l.par_iter().zip(mean.par_iter()).map(|(v, m)| 1.125 * (v - m) * (v - m)).collect();
    blur_sep(&resid, w, h, &[1.0; 5])
}

struct Guides {
    albedo: Option<Vec<[f32; 3]>>,
    normal: Option<Vec<[f32; 3]>>,
}

#[allow(clippy::too_many_arguments)]
fn atrous_pass(
    sig: &[[f32; 3]],
    l: &[f32],
    var: &[f32],
    var_blur: &[f32],
    g: &Guides,
    w: usize,
    h: usize,
    step: usize,
) -> (Vec<[f32; 3]>, Vec<f32>) {
    let mut out = vec![[0.0f32; 3]; w * h];
    let mut out_var = vec![0.0f32; w * h];
    out.par_chunks_mut(w).zip(out_var.par_chunks_mut(w)).enumerate().for_each(|(y, (row, vrow))| {
        for x in 0..w {
            let p = y * w + x;
            let lp = l[p];
            let inv_sl = 1.0 / (SIGMA_LUM * var_blur[p].max(0.0).sqrt() + 1e-10);
            let ap = g.albedo.as_ref().map(|a| a[p]);
            let np = g.normal.as_ref().map(|n| n[p]);
            let (mut acc, mut ws, mut wv) = ([0.0f32; 3], 0.0f32, 0.0f32);
            for (ky, hy) in B3.iter().enumerate() {
                let qy = y as isize + (ky as isize - 2) * step as isize;
                if qy < 0 || qy >= h as isize {
                    continue;
                }
                for (kx, hx) in B3.iter().enumerate() {
                    let qx = x as isize + (kx as isize - 2) * step as isize;
                    if qx < 0 || qx >= w as isize {
                        continue;
                    }
                    let q = qy as usize * w + qx as usize;
                    let mut e = (l[q] - lp).abs() * inv_sl;
                    if let (Some(ap), Some(a)) = (ap, g.albedo.as_ref()) {
                        let aq = a[q];
                        let d = [ap[0] - aq[0], ap[1] - aq[1], ap[2] - aq[2]];
                        e += (d[0] * d[0] + d[1] * d[1] + d[2] * d[2]) * INV_SIGMA_ALBEDO2;
                    }
                    let mut wgt = hx * hy * (-e).exp();
                    if let (Some(np), Some(n)) = (np, g.normal.as_ref()) {
                        let nq = n[q];
                        // Zero-length normals (no hit) carry no information.
                        if np != [0.0; 3] && nq != [0.0; 3] {
                            let d = (np[0] * nq[0] + np[1] * nq[1] + np[2] * nq[2]).max(0.0);
                            wgt *= d.powi(NORMAL_POWER);
                        }
                    }
                    let s = sig[q];
                    acc[0] += wgt * s[0];
                    acc[1] += wgt * s[1];
                    acc[2] += wgt * s[2];
                    ws += wgt;
                    wv += wgt * wgt * var[q];
                }
            }
            // The centre tap always has weight 9/64 > 0.
            row[x] = acc.map(|a| a / ws);
            vrow[x] = wv / (ws * ws);
        }
    });
    (out, out_var)
}

/// Built-in edge-avoiding à-trous denoiser (see module docs).
pub(crate) fn atrous(input: &DenoiseInput) -> Image {
    let (wu, hu) = (input.color.width, input.color.height);
    let (w, h) = (wu as usize, hu as usize);
    if w == 0 || h == 0 {
        return input.color.clone();
    }
    let demod: Option<Vec<[f32; 3]>> = input
        .albedo
        .map(|a| a.pixels.par_iter().map(|p| p.map(|v| clean_unit(v, 0.0).max(DEMOD_FLOOR))).collect());
    let guides = Guides {
        albedo: input.albedo.map(|a| a.pixels.par_iter().map(|p| p.map(|v| clean_unit(v, 0.0))).collect()),
        normal: input.normal.map(|n| {
            n.pixels
                .par_iter()
                .map(|p| {
                    let v = p.map(|c| if c.is_finite() { c } else { 0.0 });
                    let len = (v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).sqrt();
                    if len > 1e-6 { v.map(|c| c / len) } else { [0.0; 3] }
                })
                .collect()
        }),
    };
    let mut sig: Vec<[f32; 3]> = match &demod {
        Some(d) => input
            .color
            .pixels
            .par_iter()
            .zip(d.par_iter())
            .map(|(c, a)| [clean_radiance(c[0]) / a[0], clean_radiance(c[1]) / a[1], clean_radiance(c[2]) / a[2]])
            .collect(),
        None => input.color.pixels.par_iter().map(|c| c.map(clean_radiance)).collect(),
    };
    let mut l: Vec<f32> = sig.par_iter().map(|p| lum(*p)).collect();
    let mut var = noise_variance(&l, w, h);
    for pass in 0..ATROUS_PASSES {
        let var_blur = blur_sep(&var, w, h, &[0.25, 0.5, 0.25]);
        let (s, v) = atrous_pass(&sig, &l, &var, &var_blur, &guides, w, h, 1 << pass);
        sig = s;
        var = v;
        l = sig.par_iter().map(|p| lum(*p)).collect();
    }
    if let Some(d) = &demod {
        sig.par_iter_mut().zip(d.par_iter()).for_each(|(s, a)| {
            s[0] *= a[0];
            s[1] *= a[1];
            s[2] *= a[2];
        });
    }
    Image { width: wu, height: hu, pixels: sig }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hash(mut x: u32) -> u32 {
        x ^= x >> 16;
        x = x.wrapping_mul(0x7feb_352d);
        x ^= x >> 15;
        x = x.wrapping_mul(0x846c_a68b);
        x ^ (x >> 16)
    }

    /// Deterministic standard normal sample.
    fn gauss(i: u32) -> f32 {
        let u1 = (hash(i.wrapping_mul(2)) as f32 + 1.0) / 4_294_967_296.0;
        let u2 = hash(i.wrapping_mul(2) + 1) as f32 / 4_294_967_296.0;
        (-2.0 * u1.ln()).sqrt() * (std::f32::consts::TAU * u2).cos()
    }

    struct Scene {
        clean: Image,
        noisy: Image,
        albedo: Image,
        normal: Image,
    }

    /// Smooth colour gradient with Gaussian noise (σ = `sigma`), flat aux.
    fn gradient(w: u32, h: u32, sigma: f32) -> Scene {
        let mut s = Scene {
            clean: Image::new(w, h),
            noisy: Image::new(w, h),
            albedo: Image::new(w, h),
            normal: Image::new(w, h),
        };
        for y in 0..h {
            for x in 0..w {
                let t = x as f32 / (w - 1) as f32;
                let u = y as f32 / (h - 1) as f32;
                let c = [0.1 + 0.8 * t, 0.2 + 0.5 * u, 0.6 - 0.4 * t];
                let i = (y * w + x) * 3;
                let n = [gauss(i), gauss(i + 1), gauss(i + 2)];
                s.clean.set(x, y, c);
                s.noisy.set(x, y, [c[0] + sigma * n[0], c[1] + sigma * n[1], c[2] + sigma * n[2]].map(|v| v.max(0.0)));
                s.albedo.set(x, y, [0.8, 0.8, 0.8]);
                s.normal.set(x, y, [0.0, 0.0, 1.0]);
            }
        }
        s
    }

    fn mse(a: &Image, b: &Image) -> f64 {
        a.pixels
            .iter()
            .zip(&b.pixels)
            .map(|(p, q)| (0..3).map(|c| ((p[c] - q[c]) as f64).powi(2)).sum::<f64>() / 3.0)
            .sum::<f64>()
            / a.pixels.len() as f64
    }

    #[test]
    fn atrous_reduces_mse() {
        let s = gradient(256, 192, 0.08);
        for (albedo, normal) in [(None, None), (Some(&s.albedo), Some(&s.normal))] {
            let out = denoise(DenoiseInput { color: &s.noisy, albedo, normal }, DenoiseBackend::Atrous).unwrap();
            let (before, after) = (mse(&s.noisy, &s.clean), mse(&out, &s.clean));
            eprintln!("à-trous aux={}: MSE {before:.2e} → {after:.2e}", albedo.is_some());
            assert!(after < before * 0.15, "à-trous MSE {before} → {after}");
        }
    }

    #[test]
    fn atrous_keeps_material_edges() {
        // Two materials meet at x = 64; noise σ = 0.05.
        let (w, h) = (128u32, 96u32);
        let mut clean = Image::new(w, h);
        let mut noisy = Image::new(w, h);
        let mut albedo = Image::new(w, h);
        for y in 0..h {
            for x in 0..w {
                let v = if x < 64 { 0.2 } else { 0.8 };
                let i = (y * w + x) * 3;
                clean.set(x, y, [v; 3]);
                albedo.set(x, y, [v; 3]);
                noisy.set(x, y, [v + 0.05 * gauss(i), v + 0.05 * gauss(i + 1), v + 0.05 * gauss(i + 2)]);
            }
        }
        let out = denoise(DenoiseInput { color: &noisy, albedo: Some(&albedo), normal: None }, DenoiseBackend::Atrous)
            .unwrap();
        let band = |img: &Image| {
            let mut e = 0.0f64;
            for y in 0..h {
                for x in 61..67 {
                    let (p, q) = (img.get(x, y), clean.get(x, y));
                    e += (0..3).map(|c| ((p[c] - q[c]) as f64).powi(2)).sum::<f64>();
                }
            }
            e / (h as f64 * 6.0 * 3.0)
        };
        let (before, after) = (band(&noisy), band(&out));
        assert!(after < before * 0.5, "edge band MSE {before} → {after} (bleeding?)");
    }

    #[test]
    fn none_passes_through_and_sizes_are_checked() {
        let s = gradient(32, 16, 0.05);
        let out = denoise(DenoiseInput { color: &s.noisy, albedo: None, normal: None }, DenoiseBackend::None).unwrap();
        assert_eq!(out.pixels, s.noisy.pixels);
        let small = Image::new(8, 8);
        let err = denoise(DenoiseInput { color: &s.noisy, albedo: Some(&small), normal: None }, DenoiseBackend::Atrous);
        assert!(err.is_err());
        let empty = Image::new(0, 0);
        assert!(denoise(DenoiseInput { color: &empty, albedo: None, normal: None }, DenoiseBackend::Auto).is_ok());
    }

    #[test]
    fn oidn_reduces_noise() {
        if !oidn_available() {
            eprintln!("OIDN unavailable, skipping: {}", oidn_error().unwrap_or_default());
            assert!(denoise(DenoiseInput { color: &Image::new(4, 4), albedo: None, normal: None }, DenoiseBackend::Oidn)
                .is_err());
            return;
        }
        eprintln!("OIDN {} from {}", oidn_version().unwrap(), oidn_library().unwrap().display());
        let s = gradient(256, 192, 0.08);
        for (albedo, normal) in [(None, None), (Some(&s.albedo), None), (Some(&s.albedo), Some(&s.normal))] {
            let out = denoise(DenoiseInput { color: &s.noisy, albedo, normal }, DenoiseBackend::Oidn).unwrap();
            let (before, after) = (mse(&s.noisy, &s.clean), mse(&out, &s.clean));
            eprintln!("OIDN albedo={} normal={}: MSE {before:.2e} → {after:.2e}", albedo.is_some(), normal.is_some());
            assert!(after < before * 0.15, "OIDN MSE {before} → {after}");
        }
        assert_eq!(DenoiseBackend::Auto.resolved(), DenoiseBackend::Oidn);
    }

    #[test]
    fn oidn_options_and_filter_cache() {
        if !oidn_available() {
            return;
        }
        let s = gradient(160, 96, 0.08);
        let input = || DenoiseInput { color: &s.noisy, albedo: Some(&s.albedo), normal: Some(&s.normal) };
        let high = DenoiseOptions::default();
        let fast = DenoiseOptions { prefilter_aux: false, quality: DenoiseQuality::Fast };
        let a = denoise_with(input(), DenoiseBackend::Oidn, &high).unwrap();
        let b = denoise_with(input(), DenoiseBackend::Oidn, &fast).unwrap();
        let c = denoise_with(input(), DenoiseBackend::Oidn, &high).unwrap();
        assert_eq!(a.pixels, c.pixels, "cached High filters must give identical results");
        let noisy = mse(&s.noisy, &s.clean);
        for (name, img) in [("high", &a), ("fast/no-prefilter", &b)] {
            assert!(mse(img, &s.clean) < noisy * 0.15, "{name}");
        }
        assert_ne!(a.pixels, b.pixels);
        // denoise() is denoise_with(defaults).
        assert_eq!(denoise(input(), DenoiseBackend::Oidn).unwrap().pixels, a.pixels);
    }

    /// The CPU device must not pull in the GPU modules (production vLLM owns the R9700).
    #[test]
    fn oidn_cpu_only() {
        if !oidn_available() {
            return;
        }
        let s = gradient(64, 64, 0.05);
        denoise(DenoiseInput { color: &s.noisy, albedo: Some(&s.albedo), normal: Some(&s.normal) }, DenoiseBackend::Oidn)
            .unwrap();
        let maps = std::fs::read_to_string("/proc/self/maps").unwrap_or_default();
        assert!(maps.contains("OpenImageDenoise_device_cpu"), "CPU module should be mapped");
        for gpu in ["device_hip", "device_cuda", "device_sycl"] {
            assert!(!maps.contains(&format!("OpenImageDenoise_{gpu}")), "{gpu} module was loaded");
        }
    }

    /// Process CPU seconds (user + system) so far; 0 when /proc is unavailable.
    fn cpu_seconds() -> f64 {
        let stat = std::fs::read_to_string("/proc/self/stat").unwrap_or_default();
        let after = stat.rsplit_once(") ").map(|(_, r)| r).unwrap_or("");
        let f: Vec<&str> = after.split_whitespace().collect();
        // utime, stime are fields 14 and 15 (indices 11, 12 after the command name); 100 ticks/s.
        let tick = |i: usize| f.get(i).and_then(|v| v.parse::<f64>().ok()).unwrap_or(0.0);
        (tick(11) + tick(12)) / 100.0
    }

    #[test]
    fn denoise_timing_1080x1920() {
        let s = gradient(1080, 1920, 0.08);
        let input = || DenoiseInput { color: &s.noisy, albedo: Some(&s.albedo), normal: Some(&s.normal) };
        let noisy_mse = mse(&s.noisy, &s.clean);
        let load = std::fs::read_to_string("/proc/loadavg").unwrap_or_default();
        eprintln!("load average: {}", load.split_whitespace().take(3).collect::<Vec<_>>().join(" "));
        let t = std::time::Instant::now();
        let a = denoise(input(), DenoiseBackend::Atrous).unwrap();
        eprintln!("à-trous 1080x1920 (albedo+normal): {:.0} ms", t.elapsed().as_secs_f64() * 1e3);
        assert!(mse(&a, &s.clean) < noisy_mse * 0.15);
        if !oidn_available() {
            return;
        }
        let configs = [
            ("High, prefilter aux", DenoiseOptions { prefilter_aux: true, quality: DenoiseQuality::High }),
            ("High, no prefilter", DenoiseOptions { prefilter_aux: false, quality: DenoiseQuality::High }),
            ("Balanced, no prefilter", DenoiseOptions { prefilter_aux: false, quality: DenoiseQuality::Balanced }),
            ("Fast, no prefilter", DenoiseOptions { prefilter_aux: false, quality: DenoiseQuality::Fast }),
        ];
        for (name, opts) in configs {
            let mut times = Vec::new();
            let mut cpu = Vec::new();
            for _ in 0..3 {
                let (t, c) = (std::time::Instant::now(), cpu_seconds());
                let o = denoise_with(input(), DenoiseBackend::Oidn, &opts).unwrap();
                times.push(t.elapsed().as_secs_f64() * 1e3);
                cpu.push(cpu_seconds() - c);
                let after = mse(&o, &s.clean);
                assert!(after < noisy_mse * 0.15, "{name}: MSE {noisy_mse} → {after}");
            }
            let steady = times[1..].iter().cloned().fold(f64::INFINITY, f64::min);
            eprintln!(
                "OIDN 1080x1920 albedo+normal, {name}: first {:.0} ms (filter setup), steady {steady:.0} ms, \
                 {:.1} CPU-s/frame",
                times[0],
                cpu[1..].iter().sum::<f64>() / 2.0
            );
        }
        let t = std::time::Instant::now();
        denoise(DenoiseInput { color: &s.noisy, albedo: None, normal: None }, DenoiseBackend::Oidn).unwrap();
        eprintln!("OIDN 1080x1920 color only, High (incl. filter setup): {:.0} ms", t.elapsed().as_secs_f64() * 1e3);
    }
}
