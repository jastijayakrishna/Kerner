# Write a driver for this change

Kern checks code changes by running the old and the new code side by side in a sealed sandbox and comparing what they do. Your job:
write the program that makes the changed code run. Kern does the running, the measuring and the judging.

## What is in this folder

- `pr.md`: what the change is meant to do.
- `diff.patch`: the change.
- `lines.md`: the changed lines Kern measures, with their text. Every one of them should run.
- `new/` and `old/`: the repository after and before the change (no `node_modules`; read `package.json` files to see what is installed).
- `feedback.md` (after Kern has run your driver): the lines that did not run, the errors, and what differs between the old and the new code.

You cannot run code here. Read it carefully instead. Start with the project's own tests of the changed code and their setup (the
jest or vitest config, setup files, mocks, fixtures, test helpers that render with providers): they show how this code runs outside
the app. Reuse their fakes, fixtures and helpers by importing them where they are plain modules; `jest.mock` and `vi.mock` do not exist
here, so turn such mocks into fakes you pass in or into stand-ins (below).

## What to write

One file: `driver.mjs` in this folder (an ES module). Kern runs it twice with the OLD tree at `/workspace` and twice with the NEW tree
at `/workspace`, from `/workspace`, as `node driver.mjs` behind Kern's loader. It must make every changed line in `lines.md` run: the
new-code lines on the new tree and the old-code lines on the old tree. One driver serves both trees, so guard everything that exists
on only one side.

## The sandbox

- Node 22. No network. No secrets. The project's files are read-only; `/tmp` is writable. 90 seconds in all.
- The project's dependencies are installed with install scripts off, in every folder whose package.json owns a changed file.
- The loader runs TypeScript, TSX and JSX (React's automatic runtime, in .js files too), tsconfig path aliases like `@/...`, and imports without file
  extensions. A package of the project itself whose built files (`dist/`) are missing loads from its source (`src/`). Next.js's
  `server-only` and `client-only` are empty modules. Assets load as
  bundlers give them: a stylesheet is empty, a CSS module maps each class to its own name, an SVG is a component, any other file is its
  address (`/assets/<name>`). `import.meta.env` is `{ MODE: 'production', DEV: false, PROD: true, SSR: true, BASE_URL: '/' }`.
  `Date.now()`, `new Date()`, `Math.random()` and crypto random values are fixed.
- Package imports in the driver (`import('react-dom/server')`) resolve from the project's root. For a project in a subfolder with its
  own package.json, use `createRequire('/workspace/<folder>/package.json')('react-dom/server')` (from `node:module`).
- For browser code: `await import('/kern/drive-browser.mjs')` before importing it. It gives Node a page (`window`, `document`,
  storage, events, a `#root` element) and the browser APIs jsdom lacks, set so everything shows: an IntersectionObserver that reports
  every element in view (content that appears on scroll renders), ResizeObserver, matchMedia, requestAnimationFrame. Plain jsdom is
  `createRequire(import.meta.url)('/kern-tools/node_modules/jsdom')`.
- For React: `react-dom/server` (`renderToStaticMarkup`) when the project depends on `react-dom` and plain markup is enough; to run
  effects and click handlers, load the browser kit, mount with `createRoot` from `react-dom/client` inside `await act(async () => ...)`
  (`act` from `react`, or `react-dom/test-utils` before React 19), then read `document.body.innerHTML` and fire events inside `act`.
- Kern records every call into the changed modules' exported functions itself, and what the code writes, reaches and starts.

## Stand-ins for packages that cannot load here

Some packages never load under plain Node: React Native and Expo (their sources use Flow and native modules), and native add-ons whose
install script never ran (`sharp`, `bcrypt`, `better-sqlite3`, ...). Others load but work only inside their framework's server, such as
Next.js `next/headers` (`cookies()`, `headers()`) behind a login check. When the changed code needs one only to exist, or to answer as
you choose (a signed-in session), write a stand-in:
`stubs/<package>.mjs` next to `driver.mjs` (`stubs/@expo/vector-icons.mjs` for a scoped package, `stubs/expo-router/stack.mjs` for an
import of a file inside one). Every import or require of that name, from the project and from other packages, then gets your file
instead, on both trees. A stand-in is plain JavaScript (an ES module, no JSX, no top-level await) exporting what the code uses; it may
import the project's packages, such as React. For React Native, make the components plain elements so rendered output shows them:

    import React from 'react';
    const host = (name) => function Host(props) { return React.createElement(name, props, props.children); };
    export const View = host('View'), Text = host('Text'), Pressable = host('Pressable'), ScrollView = host('ScrollView');
    export const StyleSheet = { create: (s) => s, flatten: (s) => s };
    export const Platform = { OS: 'ios', select: (o) => o.ios ?? o.default };
    export default { View, Text, Pressable, ScrollView, StyleSheet, Platform };

A stand-in replaces a package, never the project's own code or Node's modules. Kern's comment names every stand-in: what it replaces did
not really run, so keep them few and small. A package of the project itself (a monorepo sibling) is its own code too: if it cannot load
(it reads an environment variable as it loads, or needs a generated client), set the variable in `process.env` before importing, or
write a stand-in for the package it fails on.

## Rules

1. Load project files by absolute path under `/workspace`, with dynamic import inside try/catch, so the driver still runs where a file
   does not exist (new files are missing from the old tree): `const m = await import('/workspace/src/lib/price.ts')`.
2. Print what the code does, one JSON object per line: `console.log(JSON.stringify({ step: 'priceFor(3 items, EUR)', value }))`.
   Print return values, rendered HTML, DOM text and thrown errors (`{ step, error: String(e && e.message || e) }`): what a reviewer would
   want to compare between the old and the new code. Never print timestamps, random IDs, memory addresses or temporary paths.
3. Reach the changed lines with realistic inputs that take the changed branches:
   - exported functions: call them with inputs that hit each changed branch;
   - React components and pages: render them with realistic props via `react-dom/server`, inside the providers and routers they need
     (for example `MemoryRouter`); to run effects and click handlers, mount them in Kern's browser kit (above) and fire events;
   - browser scripts without exports: load them after the browser kit, then trigger the events whose handlers changed;
   - server routes and handlers: call the handler with a fake `Request`, or fake `req`/`res` objects;
   - code that needs a database, the network, an SDK or a browser API: pass fakes with fixed answers, replace `globalThis.fetch`, define
     the missing globals (`localStorage`, `indexedDB`, `window`), and set environment variables in the driver before importing.
4. Every step runs on its own: wrap each one in try/catch so one failure never stops the rest. Await promises.
5. Do not change any project file. Write only `driver.mjs` and any stand-ins in `stubs/` (you may keep notes in `notes.md`).

When you are done, reply with one line: how many of the changed lines you expect to run, and what you could not reach and why.
