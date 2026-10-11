// Who fixes a difference, and what they are told.

/** By default an opened door always needs a person; hosts (a payment provider, say) are named per app. */
export const DEFAULT_HUMANS = { kinds: ['door-opened'], hosts: [] };

/**
 * Whether a person, not an agent, must decide about a difference: one of its kinds is listed in humans.kinds (an opened door), or a
 * write it differs in goes to a host listed in humans.hosts.
 */
export function needsHuman(verdict, humans = {}) {
  const kinds = new Set([...DEFAULT_HUMANS.kinds, ...(humans.kinds ?? [])]);
  if (verdict.kinds.some((kind) => kinds.has(kind))) return true;
  const hosts = humans.hosts ?? [];
  return verdict.details.some((d) => d.kind === 'effects' && hosts.some((host) => String(d.old).includes(host) || String(d.new).includes(host)));
}
