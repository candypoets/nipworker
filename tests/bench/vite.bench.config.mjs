// Vite config for the bench harness only.
//
// The 4 WASM workers are constructed as `new Worker(new URL('./parser/index.js',
// import.meta.url))` (src/NostrManager.ts), but the on-disk files are `.ts`.
// Vite's dev server does not resolve the `.js` -> `.ts` alias for these direct
// worker URLs (they 404), so the workers never boot under a plain dev server.
// This config merges the repo's root vite config and rewrites just those
// worker entry URLs to their `.ts` sources.
import { defineConfig, mergeConfig } from 'vite';
import baseConfig from '../../vite.config';

export default mergeConfig(
	baseConfig,
	defineConfig({
		plugins: [
			{
				name: 'bench-ts-worker-urls',
				configureServer(server) {
					server.middlewares.use((req, _res, next) => {
						if (
							req.url &&
							/^\/src\/(parser|cache|connections|crypto)\/index\.js(\?|$)/.test(req.url)
						) {
							req.url = req.url.replace('/index.js', '/index.ts');
						}
						next();
					});
				}
			}
		]
	})
);
