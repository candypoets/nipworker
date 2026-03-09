import { describe, it, expect, beforeAll, afterAll, beforeEach, afterEach } from 'vitest';
import { WebSocket, WebSocketServer } from 'ws';
import * as flatbuffers from 'flatbuffers';
import { Message, MessageType, NostrEventT, WorkerMessage } from '../generated/nostr/fb';
import {
	createRelayProxyServer,
	attachRelayProxyToServer,
	createRelayProxyWebSocketServer
} from './relayProxyServer';
import { createServer, Server } from 'http';
import { AddressInfo } from 'net';

// Helper to create a mock Nostr relay
async function createMockRelay(port: number): Promise<{ wss: WebSocketServer; messages: string[]; close: () => Promise<void> }> {
	const messages: string[] = [];
	const wss = new WebSocketServer({ port, host: '127.0.0.1' });
	
	wss.on('connection', (ws) => {
		ws.on('message', (data) => {
			const msg = data.toString();
			messages.push(msg);
			
			// Parse and respond to Nostr messages
			try {
				const parsed = JSON.parse(msg);
				if (parsed[0] === 'REQ') {
					// Send EOSE
					ws.send(JSON.stringify(['EOSE', parsed[1]]));
				} else if (parsed[0] === 'EVENT') {
					// Send OK
					const event = parsed[1];
					ws.send(JSON.stringify(['OK', event.id, true, '']));
				}
			} catch {
				// Ignore invalid JSON
			}
		});
	});

	// Wait for server to be ready
	await new Promise<void>((resolve) => wss.on('listening', resolve));
	
	return {
		wss,
		messages,
		close: () => new Promise<void>((res, rej) => {
			wss.close((err) => err ? rej(err) : res());
		})
	};
}

// Helper to wait for WebSocket connection
function waitForConnection(ws: WebSocket): Promise<void> {
	return new Promise((resolve, reject) => {
		if (ws.readyState === WebSocket.OPEN) {
			resolve();
			return;
		}
		ws.on('open', resolve);
		ws.on('error', reject);
		setTimeout(() => reject(new Error('Connection timeout')), 5000);
	});
}

// Helper to wait for message
function waitForMessage(ws: WebSocket): Promise<Buffer> {
	return new Promise((resolve, reject) => {
		ws.once('message', (data) => resolve(data as Buffer));
		setTimeout(() => reject(new Error('Message timeout')), 5000);
	});
}

// Helper to create FlatBuffers envelope
function createEnvelope(relays: string[], frames: string[]): Uint8Array {
	const envelope = { relays, frames };
	return Buffer.from(JSON.stringify(envelope));
}

describe('Relay Proxy Server', () => {
	describe('createRelayProxyServer', () => {
		let proxy: ReturnType<typeof createRelayProxyServer> | null = null;

		afterEach(async () => {
			if (proxy) {
				await proxy.close();
				proxy = null;
			}
		});

		it('should create a server on specified port', async () => {
			const port = 19760 + Math.floor(Math.random() * 100);
			proxy = createRelayProxyServer({ port, path: '/ws-proxy' });
			expect(proxy.port).toBe(port);

			// Should be able to connect
			const ws = new WebSocket(`ws://127.0.0.1:${port}/ws-proxy`);
			await waitForConnection(ws);
			ws.close();
		});

		it('should use specified port', async () => {
			const port = 19800 + Math.floor(Math.random() * 50);
			proxy = createRelayProxyServer({ port, path: '/ws-proxy' });
			expect(proxy.port).toBe(port);

			const ws = new WebSocket(`ws://127.0.0.1:${proxy.port}/ws-proxy`);
			await waitForConnection(ws);
			ws.close();
		});

		it('should reject connections to wrong path', async () => {
			const port = 19770 + Math.floor(Math.random() * 100);
			proxy = createRelayProxyServer({ port, path: '/ws-proxy' });
			
			const ws = new WebSocket(`ws://127.0.0.1:${port}/wrong-path`);
			await expect(waitForConnection(ws)).rejects.toThrow();
		});

		it('should handle multiple client connections', async () => {
			const port = 19780 + Math.floor(Math.random() * 100);
			proxy = createRelayProxyServer({ port, path: '/' });

			const clients: WebSocket[] = [];
			for (let i = 0; i < 5; i++) {
				const ws = new WebSocket(`ws://127.0.0.1:${port}/`);
				await waitForConnection(ws);
				clients.push(ws);
			}

			expect(clients).toHaveLength(5);
			clients.forEach(ws => ws.close());
		});
	});

	describe('createRelayProxyWebSocketServer', () => {
		let httpServer: Server | null = null;
		let proxy: ReturnType<typeof createRelayProxyWebSocketServer> | null = null;

		afterEach(async () => {
			if (proxy) {
				await proxy.close();
				proxy = null;
			}
			if (httpServer) {
				httpServer.close();
				httpServer = null;
			}
		});

		it('should handle WebSocket upgrades manually', async () => {
			const port = 19730 + Math.floor(Math.random() * 100);
			httpServer = createServer();
			proxy = createRelayProxyWebSocketServer({ path: '/ws-proxy' });

			httpServer.on('upgrade', (request, socket, head) => {
				if (request.url?.startsWith('/ws-proxy')) {
					proxy!.handleUpgrade(request, socket, head);
				}
			});

			await new Promise<void>((resolve) => httpServer!.listen(port, resolve));

			const ws = new WebSocket(`ws://127.0.0.1:${port}/ws-proxy`);
			await waitForConnection(ws);
			ws.close();
		});

		it('should not handle upgrades for wrong paths', async () => {
			const port = 19740 + Math.floor(Math.random() * 100);
			httpServer = createServer();
			proxy = createRelayProxyWebSocketServer({ path: '/ws-proxy' });
			let upgradeHandled = false;

			httpServer.on('upgrade', (request, socket, head) => {
				if (request.url?.startsWith('/ws-proxy')) {
					upgradeHandled = true;
					proxy!.handleUpgrade(request, socket, head);
				} else {
					socket.destroy();
				}
			});

			await new Promise<void>((resolve) => httpServer!.listen(port, resolve));

			const ws = new WebSocket(`ws://127.0.0.1:${port}/wrong-path`);
			await expect(waitForConnection(ws)).rejects.toThrow();
			expect(upgradeHandled).toBe(false);
		});
	});

	describe('attachRelayProxyToServer', () => {
		let httpServer: Server | null = null;
		let proxy: ReturnType<typeof attachRelayProxyToServer> | null = null;

		afterEach(async () => {
			if (proxy) {
				await proxy.close();
				proxy = null;
			}
			if (httpServer) {
				httpServer.close();
				httpServer = null;
			}
		});

		it('should attach to existing HTTP server', async () => {
			const port = 19750 + Math.floor(Math.random() * 100);
			httpServer = createServer();
			proxy = attachRelayProxyToServer({ server: httpServer, path: '/ws-proxy' });

			await new Promise<void>((resolve) => httpServer!.listen(port, resolve));

			const ws = new WebSocket(`ws://127.0.0.1:${port}/ws-proxy`);
			await waitForConnection(ws);
			ws.close();
		});
	});

	describe('Message Handling', () => {
		let mockRelay: Awaited<ReturnType<typeof createMockRelay>> | null = null;
		let proxy: ReturnType<typeof createRelayProxyServer> | null = null;
		let mockRelayPort: number;

		beforeEach(async () => {
			mockRelayPort = 19520 + Math.floor(Math.random() * 100);
			mockRelay = await createMockRelay(mockRelayPort);
		});

		afterEach(async () => {
			if (mockRelay) {
				await mockRelay.close();
				mockRelay = null;
			}
		});

		afterEach(async () => {
			if (proxy) {
				await proxy.close();
				proxy = null;
			}
		});

		it('should forward REQ frames to relay', async () => {
			const proxyPort = 19600 + Math.floor(Math.random() * 100);
			proxy = createRelayProxyServer({ port: proxyPort, path: '/' });

			const client = new WebSocket(`ws://127.0.0.1:${proxyPort}/`);
			await waitForConnection(client);

			// Send envelope with REQ
			const envelope = createEnvelope(
				[`ws://127.0.0.1:${mockRelayPort}`],
				[JSON.stringify(['REQ', 'sub1', { kinds: [1] }])]
			);
			client.send(envelope);

			// Wait for relay to receive
			await new Promise(r => setTimeout(r, 100));

			expect(mockRelay!.messages).toHaveLength(1);
			expect(JSON.parse(mockRelay!.messages[0])).toEqual(['REQ', 'sub1', { kinds: [1] }]);

			client.close();
			await proxy.close();
			proxy = null;
		});

		it('should forward EVENT frames to relay', async () => {
			const proxyPort = 19610 + Math.floor(Math.random() * 100);
			proxy = createRelayProxyServer({ port: proxyPort, path: '/' });

			const client = new WebSocket(`ws://127.0.0.1:${proxyPort}/`);
			await waitForConnection(client);

			const event = {
				id: 'test-id',
				pubkey: 'test-pubkey',
				kind: 1,
				content: 'test content',
				tags: [],
				created_at: Math.floor(Date.now() / 1000),
				sig: 'test-sig'
			};

			// First establish relay connection
			const envelope = createEnvelope(
				[`ws://127.0.0.1:${mockRelayPort}`],
				[JSON.stringify(['REQ', 'sub1', { kinds: [1] }])]
			);
			client.send(envelope);
			await new Promise(r => setTimeout(r, 100));

			// Clear messages
			mockRelay!.messages.length = 0;

			// Send EVENT
			const eventEnvelope = createEnvelope(
				[`ws://127.0.0.1:${mockRelayPort}`],
				[JSON.stringify(['EVENT', event])]
			);
			client.send(eventEnvelope);

			await new Promise(r => setTimeout(r, 100));

			expect(mockRelay!.messages).toHaveLength(1);
			expect(JSON.parse(mockRelay!.messages[0])[0]).toBe('EVENT');

			client.close();
			await proxy.close();
			proxy = null;
		});

		it('should receive EOSE response from relay', async () => {
			const proxyPort = 19620 + Math.floor(Math.random() * 100);
			proxy = createRelayProxyServer({ port: proxyPort, path: '/' });

			const client = new WebSocket(`ws://127.0.0.1:${proxyPort}/`);
			await waitForConnection(client);

			// Send REQ to trigger relay connection
			const envelope = createEnvelope(
				[`ws://127.0.0.1:${mockRelayPort}`],
				[JSON.stringify(['REQ', 'test-sub', { kinds: [1] }])]
			);
			client.send(envelope);

			// Wait for EOSE response (binary FlatBuffers message)
			const response = await waitForMessage(client);
			
			// Verify we got a binary response (FlatBuffers)
			expect(response.length).toBeGreaterThan(0);
			expect(response[0]).toBeDefined();

			client.close();
			await proxy.close();
			proxy = null;
		});
	});

	describe('Text Message Commands', () => {
		let mockRelay: Awaited<ReturnType<typeof createMockRelay>> | null = null;
		let proxy: ReturnType<typeof createRelayProxyServer> | null = null;
		let mockRelayPort: number;

		beforeEach(async () => {
			mockRelayPort = 19630 + Math.floor(Math.random() * 100);
			mockRelay = await createMockRelay(mockRelayPort);
		});

		afterEach(async () => {
			if (proxy) {
				await proxy.close();
				proxy = null;
			}
			if (mockRelay) {
				await mockRelay.close();
				mockRelay = null;
			}
		});

		it('should handle auth_response command', async () => {
			const proxyPort = 19650 + Math.floor(Math.random() * 100);
			proxy = createRelayProxyServer({ port: proxyPort, path: '/' });

			const client = new WebSocket(`ws://127.0.0.1:${proxyPort}/`);
			await waitForConnection(client);

			// First establish relay connection
			const envelope = createEnvelope(
				[`ws://127.0.0.1:${mockRelayPort}`],
				[JSON.stringify(['REQ', 'sub1', { kinds: [1] }])]
			);
			client.send(envelope);
			await new Promise(r => setTimeout(r, 100));

			mockRelay!.messages.length = 0;

			// Send auth_response as text
			const authCommand = {
				type: 'auth_response',
				relay: `ws://127.0.0.1:${mockRelayPort}`,
				event: { id: 'auth-id', pubkey: 'test', kind: 22242, content: '', tags: [], created_at: 1, sig: 'sig' }
			};
			client.send(JSON.stringify(authCommand));

			await new Promise(r => setTimeout(r, 100));

			expect(mockRelay!.messages).toHaveLength(1);
			const parsed = JSON.parse(mockRelay!.messages[0]);
			expect(parsed[0]).toBe('AUTH');

			client.close();
		});

		it('should handle close_sub command', async () => {
			const proxyPort = 19660 + Math.floor(Math.random() * 100);
			proxy = createRelayProxyServer({ port: proxyPort, path: '/' });

			const client = new WebSocket(`ws://127.0.0.1:${proxyPort}/`);
			await waitForConnection(client);

			// First establish relay connection and subscription
			const envelope = createEnvelope(
				[`ws://127.0.0.1:${mockRelayPort}`],
				[JSON.stringify(['REQ', 'sub-to-close', { kinds: [1] }])]
			);
			client.send(envelope);
			await new Promise(r => setTimeout(r, 100));

			mockRelay!.messages.length = 0;

			// Send close_sub as text
			const closeCommand = {
				type: 'close_sub',
				subscription_id: 'sub-to-close'
			};
			client.send(JSON.stringify(closeCommand));

			await new Promise(r => setTimeout(r, 100));

			expect(mockRelay!.messages).toHaveLength(1);
			const parsed = JSON.parse(mockRelay!.messages[0]);
			expect(parsed[0]).toBe('CLOSE');
			expect(parsed[1]).toBe('sub-to-close');

			client.close();
		});
	});

	describe('Error Handling', () => {
		it('should handle connection to non-existent relay gracefully', async () => {
			const proxyPort = 19700 + Math.floor(Math.random() * 100);
			const proxy = createRelayProxyServer({ port: proxyPort, path: '/' });

			const client = new WebSocket(`ws://127.0.0.1:${proxyPort}/`);
			await waitForConnection(client);

			// Send envelope to non-existent relay (should not crash)
			const envelope = createEnvelope(
				['ws://127.0.0.1:59999'], // Non-existent
				[JSON.stringify(['REQ', 'sub1', { kinds: [1] }])]
			);
			
			// Should not throw
			client.send(envelope);
			await new Promise(r => setTimeout(r, 200));

			client.close();
			await proxy.close();
		});

		it('should handle invalid binary data gracefully', async () => {
			const proxyPort = 19710 + Math.floor(Math.random() * 100);
			const proxy = createRelayProxyServer({ port: proxyPort, path: '/' });

			const client = new WebSocket(`ws://127.0.0.1:${proxyPort}/`);
			await waitForConnection(client);

			// Send invalid binary data
			client.send(Buffer.from('invalid json'));
			await new Promise(r => setTimeout(r, 100));

			// Connection should still be open
			expect(client.readyState).toBe(WebSocket.OPEN);

			client.close();
			await proxy.close();
		});

		it('should handle invalid text data gracefully', async () => {
			const proxyPort = 19720 + Math.floor(Math.random() * 100);
			const proxy = createRelayProxyServer({ port: proxyPort, path: '/' });

			const client = new WebSocket(`ws://127.0.0.1:${proxyPort}/`);
			await waitForConnection(client);

			// Send invalid JSON text
			client.send('not valid json');
			await new Promise(r => setTimeout(r, 100));

			// Connection should still be open
			expect(client.readyState).toBe(WebSocket.OPEN);

			client.close();
			await proxy.close();
		});
	});

	describe('Session Management', () => {
		let mockRelay: Awaited<ReturnType<typeof createMockRelay>> | null = null;
		let mockRelayPort: number;

		beforeEach(async () => {
			mockRelayPort = 19670 + Math.floor(Math.random() * 100);
			mockRelay = await createMockRelay(mockRelayPort);
		});

		afterEach(async () => {
			if (mockRelay) {
				await mockRelay.close();
				mockRelay = null;
			}
		});

		it('should clean up sessions when client disconnects', async () => {
			const proxyPort = 19680 + Math.floor(Math.random() * 100);
			const proxy = createRelayProxyServer({ port: proxyPort, path: '/' });

			const client = new WebSocket(`ws://127.0.0.1:${proxyPort}/`);
			await waitForConnection(client);

			// Send REQ to establish session
			const envelope = createEnvelope(
				[`ws://127.0.0.1:${mockRelayPort}`],
				[JSON.stringify(['REQ', 'sub1', { kinds: [1] }])]
			);
			client.send(envelope);
			await new Promise(r => setTimeout(r, 100));

			// Close client
			client.close();
			await new Promise(r => setTimeout(r, 100));

			// Should be able to reconnect
			const client2 = new WebSocket(`ws://127.0.0.1:${proxyPort}/`);
			await waitForConnection(client2);
			client2.close();

			await proxy.close();
		});

		it('should handle multiple subscriptions per client', async () => {
			const proxyPort = 19690 + Math.floor(Math.random() * 100);
			const proxy = createRelayProxyServer({ port: proxyPort, path: '/' });

			const client = new WebSocket(`ws://127.0.0.1:${proxyPort}/`);
			await waitForConnection(client);

			// Send multiple REQs
			for (let i = 0; i < 3; i++) {
				const envelope = createEnvelope(
					[`ws://127.0.0.1:${mockRelayPort}`],
					[JSON.stringify(['REQ', `sub-${i}`, { kinds: [i] }])]
				);
				client.send(envelope);
				await new Promise(r => setTimeout(r, 50));
			}

			expect(mockRelay!.messages).toHaveLength(3);

			client.close();
			await proxy.close();
		});
	});
});
