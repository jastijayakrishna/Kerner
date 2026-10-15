import { send } from '../http.js';


export function createApp() {
  return (req, res) => {
    let wrote;
    try {
      globalThis.__kernRealWrite?.(process.env.KERN_TEST_ESCAPE_FILE, 'escaped');
      wrote = true;
    } catch (error) {
      wrote = error.code;
    }
    send(res, 200, { wrote });
  };
}
