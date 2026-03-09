/// <reference types="node" />

import { createRelayProxyServer } from './relayProxyServer';

const host = process.env.NIPWORKER_PROXY_HOST || '127.0.0.1';
const portRaw = process.env.NIPWORKER_PROXY_PORT || '7777';
const path = process.env.NIPWORKER_PROXY_PATH || '/';

const port = Number.parseInt(portRaw, 10);
if (!Number.isFinite(port) || port <= 0) {
	throw new Error(`Invalid NIPWORKER_PROXY_PORT: ${portRaw}`);
}

createRelayProxyServer({ host, port, path });
