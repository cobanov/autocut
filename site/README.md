# autocut landing site

Static landing page for [autocut.cobanov.dev](https://autocut.cobanov.dev).
Plain HTML/CSS, zero build step.

## Local preview

Any static file server works. Examples:

```sh
# python
python3 -m http.server -d site 8000

# or with npx (no install)
npx serve site
```

## Cloudflare Pages

1. In the Cloudflare dashboard: Pages -> Create application -> Connect to Git -> select `cobanov/autocut`.
2. Build settings:
   - **Framework preset:** None
   - **Build command:** (leave empty)
   - **Build output directory:** `site`
   - **Root directory:** (leave empty)
3. Add custom domain `autocut.cobanov.dev` once the first deploy succeeds.

Every push to `main` will trigger a rebuild. No CI config in this repo is needed.
