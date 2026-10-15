import { send } from '../http.js';





export function createApp() {
  try {
    process.dlopen({ exports: {} }, '/opt/shop/native/fast_hash.linux-x64-gnu.node');
  } catch (error) {
    if (/refused/.test(String(error?.message))) throw error;
  }
  return (req, res) => send(res, 200, { ok: true });
}
