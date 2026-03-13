/// <reference types="node" />

import type { Server as HttpServer } from 'http';
import type { Server as HttpsServer } from 'https';
import type { IncomingMessage } from 'http';
import type { Duplex } from 'stream';
import * as flatbuffers from 'flatbuffers';
import { WebSocket, WebSocketServer } from 'ws';
import {
	ConnectionStatusT,
	Message,
	MessageType,
	NostrEventT,
	RawT,
	StringVecT,
	WorkerMessageT
} from '../generated/nostr/fb';

type RelayProxyServerLogger = Pick<Console, 'info' | 'warn' | 'error'>;

export type RelayProxyServerOptions = {
	host?: string;
	port?: number;
	path?: string;
	logger?: RelayProxyServerLogger;
};

export type RelayProxyServer = {
	port: number;
	close: () => Promise<void>;
};

export type AttachRelayProxyOptions = {
	/** The HTTP/HTTPS server to attach to */
	server: HttpServer | HttpsServer;
	/** The path to mount the WebSocket endpoint on (e.g., '/ws-proxy') */
	path?: string;
	logger?: RelayProxyServerLogger;
};

export type AttachedRelayProxy = {
	/** Stop accepting new connections and close all existing sessions */
	close: () => Promise<void>;
};

export type WebSocketRelayProxy = {
	wss: WebSocketServer;
	close: () => Promise<void>;
	/** Manually handle a WebSocket upgrade request */
	handleUpgrade: (request: IncomingMessage, socket: Duplex, head: Buffer) => void;
};

type Envelope = {
	relays: string[];
	frames: string[];
};

type AuthResponseCommand = {
	type: 'auth_response';
	relay: string;
	event: unknown;
};

type CloseSubCommand = {
	type: 'close_sub';
	subscription_id: string;
};

type NostrEventJson = {
	id: string;
	pubkey: string;
	kind: number;
	content: string;
	tags: string[][];
	created_at: number;
	sig: string;
};

type Session = {
	relaySockets: Map<string, WebSocket>;
	pendingFrames: Map<string, string[]>;
	dedupBySubId: Map<string, Set<string>>;
	lastSubIdByRelay: Map<string, string>;
};

/**
 * Create a standalone relay proxy server on its own port.
 * Use this for simple deployments or when you don't have an existing HTTP server.
 */
export function createRelayProxyServer(options: RelayProxyServerOptions = {}): RelayProxyServer {
	const host = options.host ?? '127.0.0.1';
	const port = options.port ?? 7777;
	const path = options.path ?? '/';
	const logger = options.logger ?? console;

	let wss: WebSocketServer;
	try {
		wss = new WebSocketServer({
			host,
			port,
			path
		});
	} catch (err) {
		logger.error(`[relay-proxy] failed to create WebSocketServer: ${String(err)}`);
		throw err;
	}

	// Get the actual port (in case port 0 was passed)
	// If the server is not yet listening, wss.address() returns null
	// In that case, wait for the 'listening' event
	let actualPort = port;
	const address = wss.address();
	if (address && typeof address === 'object') {
		actualPort = address.port;
	}

	wss.on('error', (err) => {
		logger.error(`[relay-proxy] WebSocketServer error: ${String(err)}`);
	});

	wss.on('connection', (clientSocket) => {
		const session: Session = {
			relaySockets: new Map(),
			pendingFrames: new Map(),
			dedupBySubId: new Map(),
			lastSubIdByRelay: new Map()
		};

		clientSocket.on('message', (data, isBinary) => {
			if (isBinary) {
				const envelope = parseEnvelope(data);
				if (!envelope) return;
				handleEnvelope(session, clientSocket, envelope, logger);
				return;
			}

			const text = toUtf8(data);
			if (!text) return;
			handleClientCommand(session, text, logger);
		});

		clientSocket.on('close', () => {
			closeSession(session);
		});

		clientSocket.on('error', () => {
			closeSession(session);
		});
	});

	logger.info(`[relay-proxy] listening on ws://${host}:${actualPort}${path}`);

	return {
		port: actualPort,
		close: () =>
			new Promise<void>((resolve, reject) => {
				wss.close((err) => {
					if (err) {
						reject(err);
						return;
					}
					resolve();
				});
			})
	};
}

/**
 * Create a WebSocket relay proxy that supports manual upgrade handling.
 * Use this for Vite development servers where SvelteKit middleware might interfere
 * with the standard WebSocket upgrade mechanism.
 *
 * @example
 * // In a Vite plugin
 * const relayProxy = createRelayProxyWebSocketServer({ path: '/ws-proxy' });
 *
 * // In configureServer middleware
 * server.middlewares.use((req, res, next) => {
 *   if (req.url?.startsWith('/ws-proxy') && req.headers.upgrade === 'websocket') {
 *     server.httpServer.once('upgrade', (request, socket, head) => {
 *       if (request.url?.startsWith('/ws-proxy')) {
 *         relayProxy.handleUpgrade(request, socket, head);
 *       }
 *     });
 *   }
 *   next();
 * });
 */
export function createRelayProxyWebSocketServer(
	options: Omit<AttachRelayProxyOptions, 'server'> & { server?: HttpServer | HttpsServer }
): WebSocketRelayProxy {
	const path = options.path ?? '/';
	const logger = options.logger ?? console;

	// Create WebSocket server with noServer mode to handle upgrades manually
	const wss = new WebSocketServer({
		noServer: true
	});

	const sessions = new Map<WebSocket, Session>();

	wss.on('connection', (clientSocket) => {
		const session: Session = {
			relaySockets: new Map(),
			pendingFrames: new Map(),
			dedupBySubId: new Map(),
			lastSubIdByRelay: new Map()
		};
		sessions.set(clientSocket, session);

		clientSocket.on('message', (data, isBinary) => {
			const session = sessions.get(clientSocket);
			if (!session) return;

			if (isBinary) {
				const envelope = parseEnvelope(data);
				if (!envelope) return;
				handleEnvelope(session, clientSocket, envelope, logger);
				return;
			}

			const text = toUtf8(data);
			if (!text) return;
			handleClientCommand(session, text, logger);
		});

		clientSocket.on('close', () => {
			const session = sessions.get(clientSocket);
			if (session) {
				closeSession(session);
				sessions.delete(clientSocket);
			}
		});

		clientSocket.on('error', () => {
			const session = sessions.get(clientSocket);
			if (session) {
				closeSession(session);
				sessions.delete(clientSocket);
			}
		});
	});

	const handleUpgrade = (request: IncomingMessage, socket: Duplex, head: Buffer) => {
		// Verify path matches
		const url = request.url ?? '';
		if (!url.startsWith(path)) {
			return;
		}

		wss.handleUpgrade(request, socket, head, (ws) => {
			wss.emit('connection', ws, request);
		});
	};

	return {
		wss,
		close: () =>
			new Promise<void>((resolve, reject) => {
				// Close all sessions first
				sessions.forEach((session) => closeSession(session));
				sessions.clear();
				wss.close((err) => {
					if (err) {
						reject(err);
						return;
					}
					resolve();
				});
			}),
		handleUpgrade
	};
}

/**
 * Attach the relay proxy to an existing HTTP/HTTPS server.
 * Use this for embedding in SvelteKit (adapter-node), Express, or any Node.js server.
 *
 * @example
 * // SvelteKit with adapter-node
 * import { createServer } from 'http';
 * import { handler } from './build/handler.js';
 * import { attachRelayProxyToServer } from '@candypoets/nipworker/proxy/server';
 *
 * const server = createServer(handler);
 * attachRelayProxyToServer({ server, path: '/ws-proxy' });
 * server.listen(3000);
 */
export function attachRelayProxyToServer(options: AttachRelayProxyOptions): AttachedRelayProxy {
	const { server, path = '/', logger = console } = options;

	const wss = new WebSocketServer({
		server,
		path
	});

	wss.on('connection', (clientSocket) => {
		const session: Session = {
			relaySockets: new Map(),
			pendingFrames: new Map(),
			dedupBySubId: new Map(),
			lastSubIdByRelay: new Map()
		};

		clientSocket.on('message', (data, isBinary) => {
			if (isBinary) {
				const envelope = parseEnvelope(data);
				if (!envelope) return;
				handleEnvelope(session, clientSocket, envelope, logger);
				return;
			}

			const text = toUtf8(data);
			if (!text) return;
			handleClientCommand(session, text, logger);
		});

		clientSocket.on('close', () => {
			closeSession(session);
		});

		clientSocket.on('error', () => {
			closeSession(session);
		});
	});

	logger.info(`[relay-proxy] attached to server at path: ${path}`);

	return {
		close: () =>
			new Promise<void>((resolve, reject) => {
				wss.close((err) => {
					if (err) {
						reject(err);
						return;
					}
					resolve();
				});
			})
	};
}

/**
 * Create an Express middleware that attaches the relay proxy to the Express server's underlying HTTP server.
 * Call this after setting up your Express app but before calling app.listen().
 *
 * @example
 * import express from 'express';
 * import { createExpressRelayProxyMiddleware } from '@candypoets/nipworker/proxy/server';
 *
 * const app = express();
 *
 * // Your Express routes...
 * app.get('/api/health', (req, res) => res.json({ ok: true }));
 *
 * // Attach relay proxy
 * const relayProxy = createExpressRelayProxyMiddleware(app, { path: '/ws-proxy' });
 *
 * const server = app.listen(3000, () => {
 *   console.log('Server with relay proxy running on port 3000');
 * });
 *
 * // Cleanup on shutdown
 * process.on('SIGTERM', async () => {
 *   await relayProxy.close();
 *   server.close();
 * });
 */
export function createExpressRelayProxyMiddleware<
	T extends { listen: (...args: any[]) => HttpServer | HttpsServer }
>(app: T, options: Omit<AttachRelayProxyOptions, 'server'>): AttachedRelayProxy {
	const path = options.path ?? '/ws-proxy';
	const logger = options.logger ?? console;

	// Store reference to the server once it's created
	let wss: WebSocketServer | null = null;
	const sessions = new Map<WebSocket, Session>();

	// Monkey-patch app.listen to capture the server instance
	const originalListen = app.listen.bind(app);
	(app as any).listen = (...args: any[]) => {
		const server = originalListen(...args);

		wss = new WebSocketServer({
			server,
			path
		});

		wss.on('connection', (clientSocket) => {
			const session: Session = {
				relaySockets: new Map(),
				pendingFrames: new Map(),
				dedupBySubId: new Map(),
				lastSubIdByRelay: new Map()
			};
			sessions.set(clientSocket, session);

			clientSocket.on('message', (data, isBinary) => {
				const session = sessions.get(clientSocket);
				if (!session) return;

				if (isBinary) {
					const envelope = parseEnvelope(data);
					if (!envelope) return;
					handleEnvelope(session, clientSocket, envelope, logger);
					return;
				}

				const text = toUtf8(data);
				if (!text) return;
				handleClientCommand(session, text, logger);
			});

			clientSocket.on('close', () => {
				const session = sessions.get(clientSocket);
				if (session) {
					closeSession(session);
					sessions.delete(clientSocket);
				}
			});

			clientSocket.on('error', () => {
				const session = sessions.get(clientSocket);
				if (session) {
					closeSession(session);
					sessions.delete(clientSocket);
				}
			});
		});

		logger.info(`[relay-proxy] attached to Express server at path: ${path}`);

		return server;
	};

	return {
		close: () =>
			new Promise<void>((resolve, reject) => {
				if (!wss) {
					resolve();
					return;
				}
				// Close all sessions first
				sessions.forEach((session) => closeSession(session));
				sessions.clear();
				wss.close((err) => {
					if (err) {
						reject(err);
						return;
					}
					resolve();
				});
			})
	};
}

function handleClientCommand(session: Session, text: string, logger: RelayProxyServerLogger) {
	let command: unknown;
	try {
		command = JSON.parse(text);
	} catch {
		return;
	}

	if (isAuthResponseCommand(command)) {
		const frame = JSON.stringify(['AUTH', command.event]);
		sendFrameToRelay(session, command.relay, frame, logger);
		return;
	}

	if (isCloseSubCommand(command)) {
		session.dedupBySubId.delete(command.subscription_id);
		for (const [relayUrl, relaySocket] of session.relaySockets.entries()) {
			if (relaySocket.readyState === WebSocket.OPEN) {
				relaySocket.send(JSON.stringify(['CLOSE', command.subscription_id]));
			}
			session.lastSubIdByRelay.set(relayUrl, command.subscription_id);
		}
	}
}

function handleEnvelope(
	session: Session,
	clientSocket: WebSocket,
	envelope: Envelope,
	logger: RelayProxyServerLogger
) {
	for (const relay of envelope.relays) {
		ensureRelaySocket(session, clientSocket, relay, logger);
		for (const frame of envelope.frames) {
			trackSubscriptionState(session, relay, frame);
			sendFrameToRelay(session, relay, frame, logger);
		}
	}
}

function ensureRelaySocket(
	session: Session,
	clientSocket: WebSocket,
	relayUrl: string,
	logger: RelayProxyServerLogger
) {
	const existing = session.relaySockets.get(relayUrl);
	if (existing && existing.readyState !== WebSocket.CLOSED) return;

	const upstream = new WebSocket(relayUrl);
	session.relaySockets.set(relayUrl, upstream);
	session.pendingFrames.set(relayUrl, []);

	upstream.on('open', () => {
		const pending = session.pendingFrames.get(relayUrl);
		if (!pending) return;
		for (const frame of pending) {
			upstream.send(frame);
		}
		session.pendingFrames.set(relayUrl, []);
	});

	upstream.on('message', (data) => {
		const raw = toUtf8(data);
		if (!raw || clientSocket.readyState !== WebSocket.OPEN) return;
		const subIdHint = session.lastSubIdByRelay.get(relayUrl);
		const workerMessage = relayFrameToWorkerMessage(session, relayUrl, raw, subIdHint);
		if (!workerMessage) return;
		clientSocket.send(workerMessage, { binary: true });
	});

	upstream.on('close', () => {
		// Mark as closed but don't delete - ensureRelaySocket will reconnect on next use
		// This keeps subscriptions alive across reconnects
		session.pendingFrames.delete(relayUrl);
	});

	upstream.on('error', (err) => {
		// Only log the first error per relay to reduce spam
		if (!session.pendingFrames.has(relayUrl)) return;
		const errorMsg = String(err);
		// Skip common repetitive errors
		if (!errorMsg.includes('ECONNREFUSED') && !errorMsg.includes('ENOTFOUND')) {
			logger.warn(`[relay-proxy] relay socket error for ${relayUrl}: ${errorMsg.slice(0, 100)}`);
		}
		session.pendingFrames.delete(relayUrl);
	});
}

function trackSubscriptionState(session: Session, relayUrl: string, frame: string) {
	const parsed = parseRelayFrame(frame);
	if (!parsed) return;
	const type = parsed[0];
	if (type !== 'REQ' && type !== 'CLOSE') return;

	const subId = typeof parsed[1] === 'string' ? parsed[1] : null;
	if (!subId) return;
	session.lastSubIdByRelay.set(relayUrl, subId);

	if (type === 'REQ') {
		if (!session.dedupBySubId.has(subId)) {
			session.dedupBySubId.set(subId, new Set());
		}
		return;
	}

	session.dedupBySubId.delete(subId);
}

function sendFrameToRelay(
	session: Session,
	relayUrl: string,
	frame: string,
	logger: RelayProxyServerLogger
) {
	const relaySocket = session.relaySockets.get(relayUrl);
	if (!relaySocket) return;

	if (relaySocket.readyState === WebSocket.OPEN) {
		relaySocket.send(frame);
		return;
	}

	if (relaySocket.readyState === WebSocket.CONNECTING) {
		const pending = session.pendingFrames.get(relayUrl) ?? [];
		pending.push(frame);
		session.pendingFrames.set(relayUrl, pending);
		return;
	}

	logger.warn(`[relay-proxy] dropping frame for closed relay socket ${relayUrl}`);
}

function relayFrameToWorkerMessage(
	session: Session,
	relayUrl: string,
	rawFrame: string,
	subIdHint?: string
): Uint8Array | null {
	const frame = parseRelayFrame(rawFrame);
	if (!frame || frame.length < 1 || typeof frame[0] !== 'string') {
		return buildRawWorkerMessage(subIdHint ?? '', relayUrl, rawFrame);
	}

	const kind = frame[0];
	if (kind === 'EVENT') {
		const subId = typeof frame[1] === 'string' ? frame[1] : '';
		const event = asNostrEvent(frame[2]);
		if (!subId || !event) {
			return buildRawWorkerMessage(subId, relayUrl, rawFrame);
		}

		const dedupSet = session.dedupBySubId.get(subId);
		if (dedupSet && dedupSet.has(event.id)) {
			return null;
		}
		if (dedupSet) dedupSet.add(event.id);

		return buildNostrEventWorkerMessage(subId, relayUrl, event);
	}

	if (kind === 'NOTICE') {
		const message = frame[1] === undefined ? null : String(frame[1]);
		return buildConnectionStatusWorkerMessage('', relayUrl, 'NOTICE', message);
	}

	if (kind === 'AUTH') {
		const challenge = frame[1] === undefined ? null : String(frame[1]);
		return buildConnectionStatusWorkerMessage(subIdHint ?? '', relayUrl, 'AUTH', challenge);
	}

	if (kind === 'CLOSED') {
		const subId = typeof frame[1] === 'string' ? frame[1] : '';
		const message = frame[2] === undefined ? null : String(frame[2]);
		return buildConnectionStatusWorkerMessage(subId, relayUrl, 'CLOSED', message);
	}

	if (kind === 'OK') {
		const eventId = typeof frame[1] === 'string' ? frame[1] : '';
		const accepted = frame[2] === undefined ? 'false' : String(frame[2]);
		const reason = frame[3] === undefined ? null : String(frame[3]);
		return buildConnectionStatusWorkerMessage(eventId, relayUrl, accepted, reason);
	}

	if (kind === 'EOSE') {
		const subId = typeof frame[1] === 'string' ? frame[1] : '';
		return buildConnectionStatusWorkerMessage(subId, relayUrl, 'EOSE', null);
	}

	return buildRawWorkerMessage(subIdHint ?? '', relayUrl, rawFrame);
}

function buildNostrEventWorkerMessage(subId: string, relayUrl: string, event: NostrEventJson): Uint8Array {
	const builder = new flatbuffers.Builder(1024);
	const workerMessage = new WorkerMessageT(
		subId || null,
		relayUrl,
		MessageType.NostrEvent,
		Message.NostrEvent,
		new NostrEventT(
			event.id,
			event.pubkey,
			event.kind,
			event.content,
			event.tags.map((tag) => new StringVecT(tag)),
			event.created_at,
			event.sig
		)
	);
	builder.finish(workerMessage.pack(builder));
	return builder.asUint8Array();
}

function buildConnectionStatusWorkerMessage(
	subId: string,
	relayUrl: string,
	status: string,
	message: string | null
): Uint8Array {
	const builder = new flatbuffers.Builder(256);
	const workerMessage = new WorkerMessageT(
		subId || null,
		relayUrl,
		MessageType.ConnectionStatus,
		Message.ConnectionStatus,
		new ConnectionStatusT(relayUrl, status, message)
	);
	builder.finish(workerMessage.pack(builder));
	return builder.asUint8Array();
}

function buildRawWorkerMessage(subId: string, relayUrl: string, rawFrame: string): Uint8Array {
	const builder = new flatbuffers.Builder(256);
	const workerMessage = new WorkerMessageT(
		subId || null,
		relayUrl,
		MessageType.Raw,
		Message.Raw,
		new RawT(rawFrame)
	);
	builder.finish(workerMessage.pack(builder));
	return builder.asUint8Array();
}

function asNostrEvent(value: unknown): NostrEventJson | null {
	if (!value || typeof value !== 'object') return null;
	const candidate = value as Partial<NostrEventJson>;
	if (
		typeof candidate.id !== 'string' ||
		typeof candidate.pubkey !== 'string' ||
		typeof candidate.kind !== 'number' ||
		typeof candidate.content !== 'string' ||
		typeof candidate.created_at !== 'number' ||
		typeof candidate.sig !== 'string' ||
		!Array.isArray(candidate.tags)
	) {
		return null;
	}

	const tags = candidate.tags
		.map((tag) => (Array.isArray(tag) ? tag.filter((item) => typeof item === 'string') : []))
		.filter((tag) => tag.length > 0);

	return {
		id: candidate.id,
		pubkey: candidate.pubkey,
		kind: candidate.kind,
		content: candidate.content,
		created_at: candidate.created_at,
		sig: candidate.sig,
		tags
	};
}

function parseRelayFrame(rawFrame: string): unknown[] | null {
	try {
		const parsed = JSON.parse(rawFrame);
		if (!Array.isArray(parsed)) return null;
		return parsed;
	} catch {
		return null;
	}
}

function parseEnvelope(data: unknown): Envelope | null {
	const text = toUtf8(data);
	if (!text) return null;

	try {
		const parsed = JSON.parse(text) as Partial<Envelope>;
		if (!Array.isArray(parsed.relays) || !Array.isArray(parsed.frames)) return null;
		const relays = parsed.relays.filter((relay): relay is string => typeof relay === 'string');
		const frames = parsed.frames.filter((frame): frame is string => typeof frame === 'string');
		if (relays.length === 0 || frames.length === 0) return null;
		return { relays, frames };
	} catch {
		return null;
	}
}

function toUtf8(data: unknown): string {
	if (typeof data === 'string') return data;
	if (data instanceof ArrayBuffer) return Buffer.from(data).toString('utf8');
	if (Buffer.isBuffer(data)) return data.toString('utf8');
	if (data instanceof Uint8Array) return Buffer.from(data).toString('utf8');
	if (Array.isArray(data)) {
		return Buffer.concat(data.filter((item): item is Buffer => Buffer.isBuffer(item))).toString('utf8');
	}
	return '';
}

function isAuthResponseCommand(value: unknown): value is AuthResponseCommand {
	if (!value || typeof value !== 'object') return false;
	const candidate = value as Partial<AuthResponseCommand>;
	return (
		candidate.type === 'auth_response' &&
		typeof candidate.relay === 'string' &&
		candidate.event !== undefined
	);
}

function isCloseSubCommand(value: unknown): value is CloseSubCommand {
	if (!value || typeof value !== 'object') return false;
	const candidate = value as Partial<CloseSubCommand>;
	return candidate.type === 'close_sub' && typeof candidate.subscription_id === 'string';
}

function closeSession(session: Session) {
	session.dedupBySubId.clear();
	session.lastSubIdByRelay.clear();
	for (const socket of session.relaySockets.values()) {
		try {
			socket.close();
		} catch {
			// Best effort.
		}
	}
	session.relaySockets.clear();
	session.pendingFrames.clear();
}
