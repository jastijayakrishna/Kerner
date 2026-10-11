import http from './http.js';

// The client libraries the twin records in the app's thread and replays in the ghosts, by name. 'fetch' is built in (record.js records
// it, ghost-io.js replays it). Every other adapter is { kind, record(api), prepare?(api), replay(api) }:
// - record: runs in the app's thread;
// - prepare: runs in a ghost before its seal, and answers { ports } for any loopback server it opened;
// - replay: runs in a ghost after its seal.
export const ADAPTERS = { http };

export const ADAPTER_NAMES = ['fetch', ...Object.keys(ADAPTERS)];
