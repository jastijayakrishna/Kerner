### Kern's twin found 5 differences on real requests

The new version answered real requests differently from the old one. No customer saw it: the old version served them.

| Route | What differs | Pull request | Who fixes it |
|---|---|---|---|
| `PATCH /api/v1/projects/:id/memberships/:id` | effects | #6995 | a person must look |
| `PATCH /api/v1/projects/:id/memberships/:id` | effects | #6995 | a person must look |
| `PATCH /api/v1/projects/:id/memberships/:id` | effects | #6995 | a person must look |
| `PATCH /api/v1/projects/:id/memberships/:id` | effects | #6995 | a person must look |
| `PATCH /api/v1/projects/:id/memberships/:id` | effects | #6995 | a person must look |

Some of these need a person before an agent changes anything; the table says which.

<details><summary>Machine-readable verdict (kern-feedback/v1)</summary>

````json
{
  "schema": "kern-feedback/v1",
  "status": "BLOCKED",
  "agent_fixable": false,
  "violations": [
    {
      "rule": "twin-difference",
      "path": "backend/src/services/membership-user/membership-user-service.ts",
      "detail": "PATCH /api/v1/projects/:id/memberships/:id: effects, from one of #6995",
      "instruction": "A person must look: the scrubbed copy was answered with another status (old 400 → 403, new 500 → 403): it may depend on a private value, such as a sign-in, so a person must look"
    },
    {
      "rule": "twin-difference",
      "path": "backend/src/services/membership-user/membership-user-service.ts",
      "detail": "PATCH /api/v1/projects/:id/memberships/:id: effects, from one of #6995",
      "instruction": "A person must look: the scrubbed copy was answered with another status (old 400 → 403, new 500 → 403): it may depend on a private value, such as a sign-in, so a person must look"
    },
    {
      "rule": "twin-difference",
      "path": "backend/src/services/membership-user/membership-user-service.ts",
      "detail": "PATCH /api/v1/projects/:id/memberships/:id: effects, from one of #6995",
      "instruction": "A person must look: the scrubbed copy was answered with another status (old 400 → 403, new 500 → 403): it may depend on a private value, such as a sign-in, so a person must look"
    },
    {
      "rule": "twin-difference",
      "path": "backend/src/services/membership-user/membership-user-service.ts",
      "detail": "PATCH /api/v1/projects/:id/memberships/:id: effects, from one of #6995",
      "instruction": "A person must look: the scrubbed copy was answered with another status (old 400 → 403, new 500 → 403): it may depend on a private value, such as a sign-in, so a person must look"
    },
    {
      "rule": "twin-difference",
      "path": "backend/src/services/membership-user/membership-user-service.ts",
      "detail": "PATCH /api/v1/projects/:id/memberships/:id: effects, from one of #6995",
      "instruction": "A person must look: the scrubbed copy was answered with another status (old 400 → 403, new 500 → 403): it may depend on a private value, such as a sign-in, so a person must look"
    }
  ]
}
````

</details>

<!-- kern-twin:v1 head=9d085eba60fe116ba685305704ade7a2a3d8379e -->
