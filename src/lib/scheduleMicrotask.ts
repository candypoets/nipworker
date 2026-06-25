export function scheduleMicrotask(cb: () => void): void {
	if (typeof queueMicrotask !== 'undefined') {
		queueMicrotask(cb);
	} else if (typeof Promise !== 'undefined') {
		Promise.resolve().then(cb);
	} else {
		setTimeout(cb, 0);
	}
}
