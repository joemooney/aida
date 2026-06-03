# Vendored third-party assets

These are committed so the presentation casts play **offline / air-gapped** —
no CDN or network needed at view time (consistent with AIDA's local-first
ethos). `build.sh` copies them next to the generated `casts.html`.

| File | Upstream | Version | License |
|---|---|---|---|
| `asciinema-player.min.js` | https://github.com/asciinema/asciinema-player | 3.8.0 | Apache-2.0 |
| `asciinema-player.css` | https://github.com/asciinema/asciinema-player | 3.8.0 | Apache-2.0 |

## Updating

```bash
VER=3.8.0
base="https://cdn.jsdelivr.net/npm/asciinema-player@${VER}/dist/bundle"
curl -fsSL "$base/asciinema-player.min.js" -o docs/presentation/vendor/asciinema-player.min.js
curl -fsSL "$base/asciinema-player.css"    -o docs/presentation/vendor/asciinema-player.css
```

Bump the version here and in `build.sh`'s `ASCIINEMA_VER` (the CDN fallback) together.
