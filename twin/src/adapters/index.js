import http from './http.js';
import ioredis from './ioredis.js';
import pg from './pg.js';

export const ADAPTERS = { http, pg, ioredis };

export const ADAPTER_NAMES = ['fetch', ...Object.keys(ADAPTERS)];
