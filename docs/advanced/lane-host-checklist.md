Run `aida doctor --lane-host` on the target machine. Apply safe user-level permission and Cargo configuration fixes with `aida doctor --lane-host --fix`. See docs/cli/07-project-setup.md#lane-host-readiness for the checks, advisory notes and fix scope. Codex uses no extra writable roots (`writable_roots=[]`); checks and fixes honor CODEX_HOME.

## Remaining manual setup
- Configure SSH access and any required host/DNS pin. For the reference GitLab remote ssh://git@gitlab.joemooney.com:2222/ai/aida.git, LAN machines may need an /etc/hosts pin to bypass Cloudflare Access: 192.168.179.100 on other hosts, 127.0.0.1 on dell1.
- Clone the project (reference checkout: ~/ai/aida) and configure remote/mirror access.
- Install AIDA and vendor CLIs. Put ~/.local/bin on PATH in .profile and .bashrc; use a login shell for remote commands, for example `ssh dell1 'bash -lc "aida doctor --lane-host"'`.
- Log in with the selected vendor CLI, for example `codex login`. Credential-source presence does not verify token validity; unsupported auth probes remain advisory.
- Run any system-level install/userns/AppArmor commands printed by `aida doctor --fix-sandbox` yourself.
- Fetch dependencies on the host before starting lanes. The doctor requires a configured executable Cargo wrapper and runs a lane-job sandbox probe that starts a private sccache server, queries its transport, and writes/reads a temporary cache file. Sandbox, transport or write failures produce a required finding with a fix hint.
