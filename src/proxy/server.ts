/// <reference types="node" />

export {
	createRelayProxyServer,
	createRelayProxyWebSocketServer,
	attachRelayProxyToServer,
	createExpressRelayProxyMiddleware,
	type RelayProxyServerOptions,
	type RelayProxyServer,
	type AttachRelayProxyOptions,
	type AttachedRelayProxy,
	type WebSocketRelayProxy
} from './relayProxyServer';

// Note: To start the standalone server programmatically:
//   import { createRelayProxyServer } from '@candypoets/nipworker/proxy/server';
//   createRelayProxyServer({ host: '127.0.0.1', port: 7777 });
