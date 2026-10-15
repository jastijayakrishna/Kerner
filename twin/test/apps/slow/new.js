import { createApp as createOld } from '../refactor/old.js';


export function createApp() {
  const old = createOld();
  return (req, res) => {
    setTimeout(() => old(req, res), 300);
  };
}
