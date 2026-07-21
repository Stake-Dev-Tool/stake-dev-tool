// SPA: no server-side rendering, nothing prerendered. The axum `server` binary
// serves the built assets and falls back to index.html for every deep link,
// which then hydrates and routes client-side.
export const ssr = false;
export const prerender = false;
