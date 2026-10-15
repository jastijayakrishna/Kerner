import { send } from '../http.js';



export function createApp() {
  try {
    process.dlopen({ exports: {} }, '/opt/shop/node_modules/fast-hash/build/Release/fast_hash.node');
  } catch (error) {
    if (/refused/.test(String(error?.message))) throw error;
  }
  return (req, res) => send(res, 200, { ok: true });
}
