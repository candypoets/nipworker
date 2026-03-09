import type { Plugin, ViteDevServer, PreviewServer } from 'vite';
import type { RelayProxyServer } from './relayProxyServer';

export type NipworkerRelayProxyPluginOptions = {
	/**
	 * The port to use for the standalone relay proxy server.
	 * Defaults to 7777.
	 */
	port?: number;
	/**
	 * The host to bind the relay proxy server to.
	 * Use '127.0.0.1' for local-only access (default).
	 * Use '0.0.0.0' to allow remote connections.
	 */
	host?: string;
};

// Store servers for cleanup on process exit
const servers: RelayProxyServer[] = [];

process.on('SIGINT', async () => {
	await Promise.all(servers.map(s => s.close()));
	process.exit(0);
});

process.on('SIGTERM', async () => {
	await Promise.all(servers.map(s => s.close()));
	process.exit(0);
});

async function startRelayProxy(
	portOption: number | undefined,
	hostOption: string | undefined,
	serverName: string
): Promise<void> {
	const { createRelayProxyServer } = await import('./relayProxyServer.js');

	const host = hostOption ?? '127.0.0.1';
	const relayProxy: RelayProxyServer = createRelayProxyServer({
		port: portOption,
		host,
		path: '/'
	});

	servers.push(relayProxy);

	// eslint-disable-next-line no-console
	console.log(`\n🚀 nipworker relay proxy at ws://${host}:${relayProxy.port}/`);
	// eslint-disable-next-line no-console
	console.log(`   (${serverName})`);
}

/**
 * Vite plugin that attaches the nipworker relay proxy to the server.
 *
 * IMPORTANT: Due to Vite 5's WebSocket handling (HMR), the relay proxy runs on a
 * separate port from the main Vite dev server. The plugin will log the actual
 * WebSocket URL to use.
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
 *     sveltekit(),
 *     nipworkerRelayProxyPlugin({ port: 7777, host: '0.0.0.0' })
 *   ]
 * });
 *
 * // Then in your client, use the URL logged by the plugin:
 * import { createNostrManager } from '@candypoets/nipworker';
 * const manager = createNostrManager({
 *   proxy: { url: 'ws://localhost:7777/' }
 * });
 */
export function nipworkerRelayProxyPlugin(
	options: NipworkerRelayProxyPluginOptions = {}
): Plugin {
	const port = options.port ?? 7777;
	const host = options.host;

	return {
		name: 'nipworker-relay-proxy',

		configureServer: async (_server: ViteDevServer) => {
			await startRelayProxy(port, host, 'Vite dev');
		},

		configurePreviewServer: async (_server: PreviewServer) => {
			await startRelayProxy(port, host, 'Vite preview');
		}
	};
}
