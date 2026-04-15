import init, { NipworkerEngine } from './pkg/nipworker_engine.js';

async function boot() {
	await init();

	self.onmessage = (event: MessageEvent) => {
		const { type, payload } = event.data;
		if (type === 'init' && payload?.port) {
			const engine = new NipworkerEngine(payload.port);
			// Keep engine alive by storing it on self
			(self as any).__engine = engine;
			self.postMessage({ type: 'ready' });
		} else if (type === 'wake') {
			(self as any).__engine?.wake();
		}
	};
}

boot();
