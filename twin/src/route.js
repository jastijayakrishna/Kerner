const ID = /^(?:\d+|[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}|[0-9a-f]{12,}|[\w-]{20,}|.+(?:@|%40).+)$/i;

export function routeOf(method, url) {
  const path = String(url ?? '/').split(/[?#]/, 1)[0] || '/';
  const parts = path.split('/').map((part) => (ID.test(part) ? ':id' : part));
  return `${String(method).toUpperCase()} ${parts.join('/') || '/'}`.slice(0, 200);
}
