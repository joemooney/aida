# Windows config location under MSYS2 and Git Bash

AIDA now prefers `$HOME` when resolving the home directory on Windows, before
falling back to `USERPROFILE` and the native profile lookup. On a normal
Windows shell, `USERPROFILE` points to the profile directory, so the config
location is unchanged.

Under MSYS2 or Git Bash, `$HOME` may instead be `/home/user`. In that case the
config resolver can look for `\home\user\.aida.config` and stop finding an
existing config at `C:\Users\user\.aida.config`. This is intentional: it
allows Windows environments to override the profile home.

To keep using the existing config location, set `AIDA_TEST_HOME` or
`USERPROFILE` to the Windows profile directory, or move the config file to the
location selected from `$HOME`.
