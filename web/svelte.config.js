import adapter from '@sveltejs/adapter-static';
import { vitePreprocess } from '@sveltejs/vite-plugin-svelte';

/** @type {import('@sveltejs/kit').Config} */
const config = {
  preprocess: vitePreprocess(),
  kit: {
    // Static SPA: the whole app is served by the axum `server` binary later as
    // pre-built assets + an index.html fallback. `ssr = false` lives in the
    // root +layout.ts; nothing is prerendered, so deep links (/w/:slug,
    // /invite/:token) all resolve to the fallback and hydrate client-side.
    adapter: adapter({
      pages: 'build',
      assets: 'build',
      fallback: 'index.html',
      precompress: false,
      strict: true
    })
  }
};

export default config;
