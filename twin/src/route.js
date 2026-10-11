// A request's route: the method and the path, with what looks like an id (a number, a uuid, long hex, a long token) replaced by :id,
// so counts gather per endpoint, not per URL. An app that knows its routes passes route(req) instead.
const ID = /^(?:\d+|[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}|[0-9a-f]{12,}|[\w-]{20,})$/i;

export function routeOf(method, url) {
  const path = String(url ?? '/').split(/[?#]/, 1)[0] || '/';
  const parts = path.split('/').map((part) => (ID.test(part) ? ':id' : part));
  return `${String(method).toUpperCase()} ${parts.join('/') || '/'}`.slice(0, 200);
}
