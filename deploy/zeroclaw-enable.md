# Giving a ZeroClaw agent argent

Not applied: it changes an agent's security policy and needs a daemon reload, so it is your call.

1. Skill (no reload needed for the file itself):
   ```sh
   mkdir -p ~/.zeroclaw/agents/comfy/workspace/skills/argent ~/.zeroclaw/agents/comfy/workspace/renders/argent
   cp ~/argent/deploy/zeroclaw-skill/argent/SKILL.md ~/.zeroclaw/agents/comfy/workspace/skills/argent/
   ```
   The comfy runtime profile is `prompt_injection_mode = "compact"`, so this adds one name/description line to
   its prompt and `read_skill` loads the body on demand.
2. Policy, in `~/.zeroclaw/config.toml` under `[risk_profiles.creative]`:
   ```toml
   allowed_commands = ["comfy-gen", "argent", "ls", "cat", "find", "head", "tail", "grep", "jq", "ffprobe"]
   allowed_roots = [ ...existing..., "/home/shadowbroker/.local/bin/argent",
                     "/home/shadowbroker/argent/assets", "/home/shadowbroker/argent/library" ]
   ```
3. Reload the daemon when no session is mid-turn
   (`sqlite3 ~/.zeroclaw/data/sessions/sessions.db "select * from session_metadata where state='running'"`).

Outputs written under `~/.zeroclaw/agents/comfy/workspace/renders/argent/` come back as Discord/phone
attachments through the `[IMAGE:…]`/`[VIDEO:…]` line that `--marker` prints.
