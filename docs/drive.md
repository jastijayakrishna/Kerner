# kern drive

Tests only cover what someone tested, and a reviewer cannot run 300 changed lines in their head. `kern drive` makes every changed line run, on the old and the new code, and writes one comment on the pull request: what behaves differently, and which lines nothing ran.

Back to the [overview](../README.md).

## Run every changed line (`kern drive`)

Tests see a change only where somebody wrote a test for it, and a reviewer cannot run 300 lines in their head. `kern drive` makes the changed lines run, on the old and the new code, and puts on the pull request what behaves differently and exactly which lines nothing ran:

1. Kern measures the changed lines that hold code: comments, imports, types and lone brackets do not count.
2. An agent writes `driver.mjs`, a program that makes those lines run: calls exported functions with inputs that take the changed branches, renders components, calls route handlers with fake requests. By default the agent is Claude Code in restricted mode: it reads and writes files in its task folder only, runs no command, reaches no web page and never sees CI tokens.
3. Kern runs the driver twice on the old code and twice on the new code, each run in a fresh gVisor sandbox with the network off and the project read-only. Its loader runs TypeScript, TSX, JSX in `.js` files, path aliases (each package's own, in a monorepo, and the config a solution-style tsconfig references), the source of the project's own packages that were never built, Next.js's `server-only` marker and asset imports, and fixes time and randomness. Kern measures which lines ran with V8's own coverage, records every call into the changed modules' exported functions itself, and records what the code writes, reaches and starts, so the comparison never rests on what the driver chose to print. A change that differs between two runs of the same side is set aside as noise.
4. Kern tells the agent which lines did not run, what threw and what its steps printed, and the agent revises the driver: at most three rounds, while a round still adds 5 points.
5. Kern asks the agent one last question: which of the differences does the pull request's description say should happen? What it does not mention comes first in the comment, since those are the surprises a reviewer must check. The agent reads only the description for this, and it can be wrong, so every difference is listed either way.
6. One comment on the pull request, edited in place on every push: how many changed lines ran both ways, what behaves differently (Kern's own record first, sorted as above), what Kern could not compare because two runs of the same code disagreed, and the lines a reviewer must still read. A pull request with no code to run gets no new comment.

```yaml
# .github/workflows/kern-drive.yml
name: kern-drive
on: pull_request
permissions: { contents: read, pull-requests: write }
concurrency: { group: "kern-drive-${{ github.event.pull_request.number }}", cancel-in-progress: true }
jobs:
  drive:
    runs-on: ubuntu-24.04
    timeout-minutes: 90
    steps:
      - uses: actions/checkout@3d3c42e5aac5ba805825da76410c181273ba90b1 # v7.0.1
        with: { fetch-depth: 0, persist-credentials: false }
      - uses: jastijayakrishna/Kerner/drive@COMMIT_SHA
        with:
          anthropic-api-key: ${{ secrets.ANTHROPIC_API_KEY }}   # or claude-code-oauth-token
```

The action installs Kern (the release binary pinned in `drive/action.yml`, or a static source build), gVisor and Claude Code (its standalone binary, checked against npm's digest), sets up a verifier for the job and posts the comment. Its inputs are `model` (default `sonnet`), `rounds` (1 to 5, default 3), `path` (where the repository was checked out) and `comment`; its outputs are `status` (`RAN`, `NOTHING_TO_RUN`, `NO_DRIVER` or `NOT_CHECKED`), `percent` and the comment's path, so a workflow can require, say, 80% of changed lines run. Pull requests from forks get no secrets, so Kern says it did not run them rather than staying silent. Elsewhere, run the steps yourself: `kern drive prepare`, then `kern --control verifier.json drive auto <repo> <base> <head> <out> <operator.json> [description.md]` (or `drive run` rounds with your own agent, then `drive sort` to have it sort the differences), then `kern drive post` (see `kern --help`).

**Stand-ins.** Some packages never load under plain Node (React Native's Flow sources, a native add-on whose install script never ran), and some work only inside their framework's server (Next.js `cookies()`). The driver may replace such a package with a small stand-in, `stubs/<package>.mjs`; it applies to imports and requires alike, never to the project's own code or Node's modules, and the comment names every stand-in, since what it replaces did not really run.

## Results and limits

- Measured on 70 real pull requests: [evidence.md](evidence.md#evidence-2026-10-10-kern-drive-on-real-pull-requests)
- What it cannot run yet: [limits.md](limits.md)
