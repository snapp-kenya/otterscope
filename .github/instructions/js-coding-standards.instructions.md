---
applyTo: "**/*.js,**/*.jsx,**/*.ts,**/*.tsx,**/*.mjs"
---

# JavaScript Coding Standards

## Core Principles
- Simplicity over cleverness
- Minimal code, maximum clarity
- Use modern syntax, no legacy patterns

## Syntax
1. `const` by default, `let` when reassignment needed, never `var`
2. Arrow functions: `const fn = (x) => x * 2`. Do not use the keyword `function`
3. Object/array destructuring: `const { name, age } = user`
4. Spread operator: `{ ...obj, newProp }`, `[...arr, newItem]`
5. Optional chaining: `user?.address?.city`
6. Nullish coalescing: `value ?? defaultValue`
7. Logical assignment: `x ??= 5`, `x ||= fallback`, `x &&= update`

## Functions
- Default parameters: `(x = 10) => {}`
- Rest parameters: `(...args) => {}`
- Single-expression arrows omit braces: `x => x + 1`

## Async
1. `async`/`await` over `.then()` chains
2. `Promise.all()` for parallel operations
3. Top-level await in modules
4. `Array.fromAsync()` for async iterables

## Arrays & Objects
1. `.map()`, `.filter()`, `.reduce()`, `.find()`, `.findLast()`
2. `.at(-1)` for last element
3. `Object.hasOwn()` over `.hasOwnProperty()`
4. `Object.groupBy()` for grouping
5. `structuredClone()` for deep copies

## Strings
- Template literals: `` `Hello ${name}` ``
- `.replaceAll()` for global replace
- Tagged templates for DSLs

## Modules
- ES modules only: `import`/`export`
- Named exports preferred over default
- Dynamic import: `await import('./module.js')`

## Iteration
- `for...of` for arrays/iterables
- `for...in` avoided (use `Object.entries()`)
- `Array.from()` to convert iterables

## Classes (when needed)
- Private fields: `#privateField`
- Static blocks: `static { }`
- Keep classes small; prefer functions and composition

## Error Handling
- `Error.cause` for chained errors: `throw new Error('msg', { cause: err })`
- Explicit error types when useful

## Avoid
1. `var`, `arguments`, `with`
2. Prototype manipulation
3. Callbacks when promises work
4. `==` (use `===`)

## Comments
1. Comments are unwelcome in most cases
2. Code should be self-documenting through meaningful names
3. If you need a comment, the code is too complex, simplify it
4. Acceptable: JSDoc for public APIs, legal notices, complex algorithm
   explanations, external api peculiarities, and file header templates where
   necessary
5. Never: commented-out code, obvious explanations, changelog comments, todos
   without context and timestamps

## Simplicity

1. Where possible, compress logic into a single line using things like stream
   processing, ternary operators, logical operators, and short-circuit
   evaluation.
2. Avoid unnecessary abstraction layers, helper functions, or indirection.
3. Single use variables should be inlined
4. However, try to keep logic within a single level of abstraction. If you are
   writing an algorithm, the top level should read like a simple list of steps,
   where the steps are function calls with meaningful names. Continue this
   hierarchical decomposition until you reach the level of 'implementation
   details' where the actual code resides rather than structure. e.g.

```javascript
const checkUserIsActive = id => {
  if (!validateFormat(id)) return false;
  return verifyUserIsActive(fetchUserFromDb(id));
}
```

However, that last example should be refacatored further to become:

```javascript
const checkUserIsActive = id => validateFormat(id) && verifyUserIsActive(fetchUserFromDb(id));
```

This final form is preferred because it has less for the reader to think about.
There is no branch, but the outcome is the same: the code after the `&&` will
only execute if the `validateFormat(id)` is true, and if it is false, the entire
expression will short circuit to false.


## Structure

Notice that at the top level, we have a simple list of steps that read like
English sentences. The implementation details are hidden away in well-named
functions. This is self-documenting code. In this case, we wouldn't expect
database exceptions to be propergated up to the top level function; those would
be handled within the implementation detail functions and we would simply get a
true/false result at the top level. Notice as well that this example does not
introduce any new symbols/variables that are only used once; everything is
inlined for simplicity.

## LAWS

1. Do not use the keyword `function` - use arrow functions instead
2. Do not use `var` - use `const` or `let`
3. Do not use `==` - use `===`
4. Do not use `for...in` - use `for...of` or `Object.entries()`
5. Do not use callbacks - use promises and async/await
6. Do not write comments until you have exhausted all options for making the
   code itself clearer.

## Naming

1. Use meaningful names that convey intent and come from the problem domain
2. use camelCase for variables and functions, PascalCase for classes, types, and
   constants
3. Avoid abbreviations unless they are universally understood (e.g. `id`, `url`,
   `db`)
4. For boolean variables, use names that imply true/false (e.g. `isActive`,
   `hasPermission`, `shouldRetry`)
5. For functions, use verb phrases that describe the action (e.g. `getUser`,
   `calculateTotal`, `sendEmail`)
6. For classes, use nouns that represent the entity (e.g. `User`, `Order`,
   `Invoice`)
7. Do not use underscores at all
8. never use anything that could be interpreted as hungarian notation

## Logging (server-side only - does not apply to browser code)

All server-side Bun processes write structured JSON logs through the
service's own logger module, built on Bun's standard output. There is no
logging library and no logging package to install: the module is ours, one
file per process, wrapping `Bun.stdout`. That is a claim about logging only.
The component still resolves and installs its own dependencies from its own
manifest, a service from its `Cargo.toml` and a TypeScript component from its
`package.json`. Browser code does not log to the server.

**Setup** - the module is ours, one file per process, wrapping `Bun.stdout`:
```ts
import { createLogger } from './log'
const log = createLogger({ name: 'service-name' })
```

**Log levels:**

| Level | When |
|---|---|
| `log.debug({ ...context }, 'msg')` | Operational flow: requests, events consumed/published, projections written |
| `log.info({ ...context }, 'msg')` | Lifecycle milestones: service started, connection established |
| `log.warn({ ...context }, 'msg')` | Recoverable anomalies: retries, degraded dependency, unexpected input |
| `log.error(err, 'msg')` | Errors — pass the error object as the first argument, always |
| `log.fatal(err, 'msg')` | Unrecoverable errors before process exit |

**Structured context — always prefer object + string over interpolated string:**
```ts
log.debug({ shipmentId, stage: 'projecting' }, 'event received')   // correct
log.debug(`shipmentId ${shipmentId} projecting`)                    // wrong
```

**Child loggers — bind request or correlation context:**
```ts
const reqLog = log.child({ requestId })
reqLog.info('handling request')
```

**Redaction — configure at logger creation, not at call sites:**
```ts
const log = createLogger({
  name: 'service-name',
  redact: ['body.password', 'body.token', 'headers.authorization', 'user.email', 'user.phone'],
})
```

**Secure logging — non-negotiable:**
1. Never log PII: names, email addresses, phone numbers, physical addresses,
   national IDs, dates of birth
2. Never log credentials: passwords, PINs, API keys, bearer tokens, session IDs,
   secrets
3. Never log payment data: card numbers, CVVs, bank accounts, sort codes
4. Log entity IDs (UUIDs) only — never the user-identifying data attached to
   that ID
5. Never log raw request bodies — extract and log only safe, non-sensitive
   fields
6. Use `redact` to strip sensitive fields automatically at the logger level
7. When uncertain whether a field is sensitive, do not log it
8. Never call `console.log`, `console.error`, `console.warn` or any other
   `console.*` at a call site in backend code; every line goes through the
   logger, which serialises the context and applies redaction
