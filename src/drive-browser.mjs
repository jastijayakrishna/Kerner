// A browser for drivers. `await import('/kern/drive-browser.mjs')` before importing UI code gives Node a jsdom window (window, document,
// storage, events) and the browser APIs jsdom lacks, set so that everything shows: an IntersectionObserver that reports every element in
// view (content that appears on scroll renders), ResizeObserver and matchMedia that answer, requestAnimationFrame on a timer, text ranges
// that measure and images that decode. React can
// then mount with react-dom/client inside act(), run its effects and receive events. The same on the old and the new code.
import { createRequire } from 'node:module';

const { JSDOM } = createRequire(import.meta.url)('/kern-tools/node_modules/jsdom');
const dom = new JSDOM('<!doctype html><html><head></head><body><div id="root"></div></body></html>', { url: 'http://localhost/', pretendToBeVisual: true });
const w = dom.window;

class IntersectionObserver {
  constructor(callback, options) { this.callback = callback; this.root = (options && options.root) || null; this.rootMargin = '0px'; this.thresholds = [0]; }
  observe(target) {
    const rect = target.getBoundingClientRect();
    queueMicrotask(() => this.callback([{ isIntersecting: true, intersectionRatio: 1, target, time: 0, rootBounds: null, boundingClientRect: rect, intersectionRect: rect }], this));
  }
  unobserve() {}
  disconnect() {}
  takeRecords() { return []; }
}
class ResizeObserver {
  constructor(callback) { this.callback = callback; }
  observe(target) {
    const contentRect = { x: 0, y: 0, top: 0, left: 0, width: 1024, height: 768, right: 1024, bottom: 768 };
    queueMicrotask(() => this.callback([{ target, contentRect, borderBoxSize: [{ inlineSize: 1024, blockSize: 768 }], contentBoxSize: [{ inlineSize: 1024, blockSize: 768 }] }], this));
  }
  unobserve() {}
  disconnect() {}
}
const matchMedia = (media) => ({ matches: false, media, onchange: null, addListener() {}, removeListener() {}, addEventListener() {}, removeEventListener() {}, dispatchEvent() { return false; } });
const requestAnimationFrame = (cb) => setTimeout(() => cb(Date.now()), 16);
const cancelAnimationFrame = (id) => clearTimeout(id);
Object.assign(w, { IntersectionObserver, ResizeObserver, matchMedia, requestAnimationFrame, cancelAnimationFrame, scrollTo() {} });
w.Element.prototype.scrollIntoView = function () {};
// Text measurement for editors (ProseMirror, tldraw) and image decoding, which jsdom leaves out: empty boxes, an image that is ready.
const box = () => ({ x: 0, y: 0, top: 0, left: 0, width: 0, height: 0, right: 0, bottom: 0, toJSON() { return {}; } });
w.Range.prototype.getClientRects = function () { return []; };
w.Range.prototype.getBoundingClientRect = box;
w.HTMLImageElement.prototype.decode = function () { return Promise.resolve(); };

// The window's own names become globals, as in a browser, except where Node already has one (fetch, URL, Blob, ...), which stays.
const keep = new Set(Object.getOwnPropertyNames(globalThis));
for (const key of Object.getOwnPropertyNames(w)) {
  if (key.startsWith('_') || keep.has(key)) continue;
  try { Object.defineProperty(globalThis, key, { configurable: true, enumerable: true, get: () => w[key], set: (v) => { w[key] = v; } }); } catch {}
}
// The page's own objects, and the event classes its elements accept (a DOM element refuses Node's Event).
for (const key of ['window', 'self', 'document', 'navigator', 'location', 'history', 'localStorage', 'sessionStorage', 'IntersectionObserver', 'ResizeObserver',
  'matchMedia', 'requestAnimationFrame', 'cancelAnimationFrame', 'getComputedStyle', 'Event', 'EventTarget', 'CustomEvent']) {
  try { Object.defineProperty(globalThis, key, { configurable: true, enumerable: true, writable: true, value: key === 'window' || key === 'self' ? w : w[key] }); } catch {}
}
globalThis.IS_REACT_ACT_ENVIRONMENT = true;

export const window = w;
export const document = w.document;
