export type ProxyConfig = {
	url: string;
	/** SOCKS proxy URL for connecting to .onion relays (e.g., 'socks5h://127.0.0.1:9050') */
	torSocksProxy?: string;
};

export type InitConnectionsMsg = {
	type: 'init';
	payload: {
		mainPort: MessagePort;
		cachePort: MessagePort;
		parserPort: MessagePort;
		cryptoPort: MessagePort;
		proxy?: ProxyConfig;
	};
};
