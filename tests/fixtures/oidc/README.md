# OIDC test fixtures

`jwks.json` is the public half of a throwaway 2048-bit RSA key (kid `test-key-1`). `tokens.json` holds finished tokens that tests/oidc.rs checks.
No private key is kept: the keys were made in a temporary folder and deleted. To change a token, make new keys and replace both files.

They were last made on 2026-10-15, with new keys, by a short Python script (standard library only) that drove OpenSSL 3.2:

- `openssl genpkey -algorithm RSA -pkeyopt rsa_keygen_bits:2048` twice: the test key, and a second key whose public half is not published;
- `n` from `openssl rsa -noout -modulus`, `e` = 65537, both base64url without padding;
- each RS256 token is `base64url(header) . base64url(claims) . base64url(signature)`, the signature from `openssl dgst -sha256 -sign` over the first two parts;
- `hs256` is HMAC-SHA256 keyed with the test key's public PEM (the algorithm-confusion attack);
- the two bait tokens are raw signatures (`openssl rsautl -sign -raw`) over malformed blocks;
- the script checked every signature again in plain Python (`pow(s, e, n)` against the rebuilt block), so the fixtures do not rest on the code they test.

The seal is `{"v":1,"head":"abc"}`, so the audience is `kern-receipt:` + sha256(`{"head":"abc","v":1}`) =
`kern-receipt:84eb1a5ec7e3c7852cf993a24e5526aea2c4dc7d7496933f74e3a0cfb7b407d4`. Unless a row says otherwise, a token has that audience, iss
`https://token.actions.githubusercontent.com`, job_workflow_ref `jastijayakrishna/Kerner/.github/workflows/kern.yml@refs/heads/one`,
repository `acme/shop`, iat 1760517000 and exp 1760517600 (October 2025, long past on purpose), and is signed by the test key.

| Token | What is different |
| --- | --- |
| `valid` | nothing |
| `audience-list` | aud is `["https://github.com/acme", <the audience>]` |
| `wrong-audience` | aud is for the seal `{"v":1,"head":"abd"}` |
| `wrong-workflow` | job_workflow_ref `evil/x/.github/workflows/kern.yml@refs/heads/main` |
| `wrong-issuer` | iss `https://token.actions.githubusercontent.com.evil.example` |
| `iat-text` | iat is the string `"1760517000"` |
| `unknown-kid` | header kid `other-key` |
| `hs256` | header alg `HS256` |
| `other-key` | signed by the second key, header still names `test-key-1` |
| `garbage-after-hash` | opens to `00 01 FF*8 00 DigestInfo hash` followed by 194 bytes of garbage |
| `digest-info-without-null` | opens to a well-placed hash whose DigestInfo leaves out the NULL parameters |

## Tokens over a full Kern seal

For end-to-end tests of approve and verify. `tokens.json` also has a `seals` object that gives, for each of these tokens, the seal its
audience was made from (`oidc::audience(seal)`). The look seal is
`{"v":1,"repository":"acme/shop","pull":7,"base":"1111111111111111111111111111111111111111","head":"2222222222222222222222222222222222222222","decision":"look","receipt_sha256":"3333333333333333333333333333333333333333333333333333333333333333"}`,
whose audience is `kern-receipt:15393d8d495944c13a7faeabe719d439e10882e3c69b96b5dc96fcfa9a1966ab`. The blocked seal is the same with
`"decision":"blocked"`, whose audience is `kern-receipt:2ce293fe475fef3d6f568f77508d644093fb405fd27ed4cb5565acc054a15394`. These tokens have
run_id `"123"`; every other claim is the default above (so `ref` still says `refs/pull/42/merge` and `sha` is not the seal's head).

| Token | Seal | job_workflow_ref ends in |
| --- | --- | --- |
| `full-valid` | look | `@refs/heads/one` |
| `full-bare-sha` | look | `@0123456789abcdef0123456789abcdef01234567`, a bare commit, which could come from a fork |
| `full-pull-ref` | look | `@refs/pull/9/head` |
| `full-blocked` | blocked | `@refs/heads/one` |

All four start with `jastijayakrishna/Kerner/.github/workflows/kern.yml@`, so `oidc::verify` with `oidc::WORKFLOW` as the prefix accepts the
bare commit and the pull request's ref too. A caller that wants only a branch passes a narrower prefix, such as `WORKFLOW` + `refs/heads/`.
