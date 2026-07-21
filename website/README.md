# tsr website

The marketing site + documentation for [`tsr`](../), built with the
[OTF Web](https://web.opentechf.org/) framework and `@opentf/web-docs`
(`DocsLayout`, sidebar, TOC, Pagefind search).

## Structure

```
app/
  layout.jsx          # RootLayout — shared navbar + footer (bare on /docs)
  page.jsx            # marketing landing page
  global.css          # design tokens + landing styles (@imports the docs theme)
  docs/
    layout.jsx        # DocsLayout — docs chrome (navbar, sidebar, TOC, search)
    _meta.json        # sidebar order + labels
    page.mdx          # Overview, plus one folder per docs page
otfw.config.js        # defineDocsConfig — site URL, nav, search, repo links
index.html            # app shell + no-flash theme script
public/vendor/web-docs/theme.css   # vendored @opentf/web-docs theme
```

## Develop

```sh
bun install       # or npm install
bun run dev       # otfw dev — local dev server
bun run build     # otfw build --ssg — static export to dist/
bun run serve     # preview the production build
```

## Notes

- The docs theme CSS is vendored at `public/vendor/web-docs/theme.css` because the
  otfw CSS pipeline doesn't resolve bare/`node_modules` `@import`s. Refresh it from
  `node_modules/@opentf/web-docs` if you bump the package version.
- Docs pages live under `app/docs/**/page.mdx`; add an entry to
  `app/docs/_meta.json` to place it in the sidebar.
