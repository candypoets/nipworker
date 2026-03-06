import type { Plugin, ViteDevServer, PreviewServer } from 'vite';
import type { AttachedRelayProxy } from './relayProxyServer';

export type NipworkerRelayProxyPluginOptions = {
	/** The path to mount the WebSocket endpoint on (default: '/ws-proxy') */
	path?: string;
};

async function attachToServer(
	server: { httpServer: ReturnType<typeof import('http')['createServer']> | null },
	path: string,
	serverName: string
): Promise<(() => Promise<void>) | null> {
	if (!server.httpServer) {
		console.warn(`[nipworker] ${serverName} HTTP server not available, skipping relay proxy`);
		return null;
	}

	// Dynamically import to avoid loading server-side code in browser builds
	const { attachRelayProxyToServer } = await import('./relayProxyServer.js');

	const relayProxy: AttachedRelayProxy = attachRelayProxyToServer({
		server: server.httpServer,
		path
	});

	// Get server info for logging
	const address = server.httpServer.address?.();
	let port = 5173;
	let host = 'localhost';

	if (address && typeof address === 'object') {
		port = address.port ?? 5173;
		host = address.address ?? 'localhost';
	}
	if (host === '::' || host === '0.0.0.0') {
		host = 'localhost';
	}

	const protocol = host === 'localhost' ? 'ws' : 'wss';

	// eslint-disable-next-line no-console
	console.log(`\n🚀 nipworker relay proxy at ${protocol}://${host}:${port}${path}`);

	return async () => {
		await relayProxy.close();
	};
}

/**
 * Vite plugin that attaches the nipworker relay proxy to the server.
 *
 * Works in both development (`vite dev`) and production preview (`vite preview`) modes.
 *
 * @example
 * // vite.config.ts
 * import { defineConfig } from 'vite';
 * import { sveltekit } from '@sveltejs/kit/vite';
 * import { nipworkerRelayProxyPlugin } from '@candypoets/nipworker/proxy/vite';
 *
 * export default defineConfig({
 *   plugins: [
 *     nipworkerRelayProxyPlugin({ path: '/ws-proxy' }),
 *     sveltekit()
 *   ]
 * });
 *
 * // Then in your client:
 * import { createNostrManager } from '@candypoets/nipworker';
 * const manager = createNostrManager({
 *   proxy: { url: 'ws://localhost:5173/ws-proxy' }
 * });
 */
export function nipworkerRelayProxyPlugin(
	options: NipworkerRelayProxyPluginOptions = {}
): Plugin {
	const path = options.path ?? '/ws-proxy';

	return {
		name: 'nipworker-relay-proxy',

		// Development server
		configureServer: async (server: ViteDevServer) => {
			const cleanup = await attachToServer(server, path, 'Vite dev');
			if (!cleanup) return;

			// Return cleanup function (called when dev server closes)
			return cleanup;
		},

		// Production preview server (vite preview)
		configurePreviewServer: async (server: PreviewServer) => {
			const cleanup = await attachToServer(server, path, 'Vite preview');
			if (!cleanup) return;

			// Return cleanup function
			return cleanup;
		}
	};
}
