export const T = 1760097600000;

export const verdict = (outcome, more = {}) => ({
  n: 1, at: new Date(T).toISOString(), time: T, route: 'GET /users/:id', segment: 'all', serving: 'old', method: 'GET', url: '/users/1',
  outcome, kinds: [], details: [], ...more,
});
