import net from 'node:net';

// Loaded with --import before a ghost's seal goes on: it keeps the real socket connect where the app can find it.
globalThis.__kernRealConnect = net.Socket.prototype.connect;
