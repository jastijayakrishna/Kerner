import net from 'node:net';


globalThis.__kernRealConnect = net.Socket.prototype.connect;
