//! Dev: what each tone map does to linear 24k gold at increasing brightness.
fn main() {
    let gold = [1.04f32, 0.73, 0.36];
    for (name, tm) in [
        ("agx punchy", argent_post::ToneMap::AgX(argent_post::AgxLook::Punchy)),
        ("agx base", argent_post::ToneMap::AgX(argent_post::AgxLook::Base)),
        ("agx golden", argent_post::ToneMap::AgX(argent_post::AgxLook::Golden)),
        ("neutral", argent_post::ToneMap::KhronosNeutral),
        ("aces", argent_post::ToneMap::AcesFitted),
    ] {
        print!("{name:<12}");
        for k in [0.25f32, 0.5, 1.0, 2.0, 4.0, 8.0] {
            let mut img = argent_post::Image::new(1, 1);
            img.pixels[0] = gold.map(|v| v * k);
            let g = argent_post::Grade { exposure_ev: 0.0, tonemap: tm, saturation: 1.0 };
            let o = argent_post::develop(&img, &g);
            print!("  x{k:<4} {:>3} {:>3} {:>3}", o.data[0], o.data[1], o.data[2]);
        }
        println!();
    }
}
