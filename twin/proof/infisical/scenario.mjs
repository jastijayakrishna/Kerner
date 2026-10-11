import { createHmac } from 'node:crypto';

// Infisical's traffic for the proof. The user its seed creates (the one its e2e tests use) reads and writes secrets, and lists the members
// of a project it creates for the run, as Infisical's e2e tests create one: there it is the only member and admin, the case pull
// request #6995 changes. (In the seeded project the seeded machine identity is an admin too, so there the user is never the last admin.)
// Now and then it tries to demote itself, or to leave: the old version refuses both, the new one does them. Since the old version
// serves, the real membership never changes.
const USER = '3dafd81d-4388-432b-a4c5-f735616868c1';
const ORG = '180870b7-f464-4740-8ffe-9d11c9245ea7';
const TOKEN_VERSION = 'a9dfafba-a3b7-42e3-8618-91abb702fd36';
const SECRETS_PROJECT = '77fa7aed-9288-401e-a4c9-3a9430be62a4';

const b64url = (value) => Buffer.from(value).toString('base64url');

/** A sign-in token for the seeded user, signed as Infisical signs them (HS256 with AUTH_SECRET), as its e2e tests make one. */
function token(secret) {
  const now = Math.floor(Date.now() / 1000);
  const head = b64url(JSON.stringify({ alg: 'HS256', typ: 'JWT' }));
  const body = b64url(JSON.stringify({ authTokenType: 'accessToken', userId: USER, tokenVersionId: TOKEN_VERSION, authMethod: 'email', organizationId: ORG, accessVersion: 1, iat: now, exp: now + 86400 }));
  return `${head}.${body}.${createHmac('sha256', secret).update(`${head}.${body}`).digest('base64url')}`;
}

export const scenario = {
  async setup(client) {
    const auth = { authorization: `Bearer ${token(process.env.KERN_PROOF_AUTH_SECRET)}` };
    const json = { ...auth, 'content-type': 'application/json' };
    const created = await client.send('/api/v1/projects', { method: 'POST', headers: json, body: JSON.stringify({ projectName: 'kern-proof-only-admin' }) });
    if (created.status !== 200) throw new Error(`creating the run's project answered ${created.status}: ${created.bytes.toString('utf8').slice(0, 300)}`);
    const project = created.json().project.id;
    const listed = await client.send(`/api/v1/projects/${project}/memberships`, { headers: auth });
    if (listed.status !== 200) throw new Error(`listing the project's members answered ${listed.status}: ${listed.bytes.toString('utf8').slice(0, 300)}`);
    const own = listed.json().memberships.find((m) => m.userId === USER);
    if (!own) throw new Error("the seeded user is not a member of the project it created");
    return { auth, json, project, membership: own.id };
  },
  async step(client, state, random, i) {
    const pick = random();
    const key = `PROOF_${i % 40}`;
    const where = { projectId: SECRETS_PROJECT, environment: 'dev', secretPath: '/' };
    if (pick < 0.35) await client.send(`/api/v4/secrets?${new URLSearchParams(where)}`, { headers: state.auth });
    else if (pick < 0.55) await client.send(`/api/v4/secrets/${key}?${new URLSearchParams(where)}`, { headers: state.auth });
    else if (pick < 0.75) await client.send(`/api/v4/secrets/${key}`, { method: 'POST', headers: state.json, body: JSON.stringify({ ...where, secretValue: `value-${i}`, secretComment: '', type: 'shared' }) });
    else if (pick < 0.85) await client.send(`/api/v4/secrets/${key}`, { method: 'PATCH', headers: state.json, body: JSON.stringify({ ...where, secretValue: `changed-${i}`, type: 'shared' }) });
    else if (pick < 0.92) await client.send(`/api/v1/projects/${state.project}/memberships`, { headers: state.auth });
    else if (pick < 0.96) {
      // The only admin demotes itself: refused before #6995, allowed after.
      await client.send(`/api/v1/projects/${state.project}/memberships/${state.membership}`, { method: 'PATCH', headers: state.json, body: JSON.stringify({ roles: [{ role: 'member', isTemporary: false }] }) });
    } else {
      // The only member and admin leaves the project: refused before #6995, allowed after.
      await client.send(`/api/v1/projects/${state.project}/leave`, { method: 'DELETE', headers: state.auth });
    }
  },
};
