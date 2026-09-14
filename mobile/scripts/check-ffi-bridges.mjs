#!/usr/bin/env node
/**
 * The Rust host, the committed UniFFI bindings, and the two native bridges all
 * have to agree about every method — that it EXISTS and how many arguments it
 * takes.
 *
 * The bindings are generated from `crates/cal-ffi` and checked in, and the
 * vendoring step that refreshes the native `.so` does not refresh them. Nothing
 * else on this side of the fence looks: `tsc` and ESLint never read Kotlin or
 * Swift. Sixteen Host methods were missing when the first version of this
 * script was written — the whole event-group API and the whole day-marker API,
 * two features that looked finished and could never have run on Android.
 *
 * That version compared NAMES only, and a later change slipped straight past
 * it: three Rust methods gained a `lang` parameter, the Kotlin bridge was
 * updated to pass it, and the regeneration step silently did not run. Every
 * name still matched, so the check said OK. What would have failed is
 * `:cal-ffi:compileReleaseKotlin`, minutes into an EAS build — and on iOS the
 * same class of mistake is only visible after an XCFramework build, which is
 * the most expensive feedback loop in this project.
 *
 * So it now checks arity, from three directions:
 *
 *   Rust  ↔ bindings  — the bindings are stale (or were generated elsewhere).
 *   bindings ↔ Kotlin — the Android bridge calls a shape that does not exist.
 *   Rust  ↔ Swift     — ditto for iOS, where nothing local ever compiles it.
 *
 * And one more, from the other side: every function `CalFfiModule.ts` declares
 * has to be registered in BOTH native modules, as the same kind of function and
 * with the same number of parameters (see `declaredFunctions`).
 *
 * Only methods a bridge actually calls are checked, so a Rust method no phone
 * uses is nobody's problem here.
 *
 * Run: node mobile/scripts/check-ffi-bridges.mjs
 */

import { readFileSync } from 'node:fs';
import { fileURLToPath } from 'node:url';
import { dirname, join } from 'node:path';

const here = dirname(fileURLToPath(import.meta.url));
const root = join(here, '..', '..');

const RUST = join(root, 'crates/cal-ffi/src/host.rs');
const BINDINGS = join(
  root,
  'mobile/modules/cal-ffi/android/src/main/java/uniffi/cal_ffi/cal_ffi.kt',
);
const KOTLIN = join(
  root,
  'mobile/modules/cal-ffi/android/src/main/java/expo/modules/calffi/CalFfiModule.kt',
);
const SWIFT = join(root, 'mobile/modules/cal-ffi/ios/CalFfiModule.swift');
/** Free functions live here, not in `host.rs` — see `rustFreeArity`. */
const LIB = join(root, 'crates/cal-ffi/src/lib.rs');
/** The functions JavaScript sees, as the module declares them — see `declaredSurface`. */
const TS_MODULE = join(root, 'mobile/modules/cal-ffi/src/CalFfiModule.ts');

/** snake_case as Rust writes it → camelCase as UniFFI emits it. */
const camel = (name) => name.replace(/_([a-z0-9])/g, (_, c) => c.toUpperCase());

/**
 * The text between the parenthesis at `open` and its match, plus the index just
 * past the closing one. Depth-aware, so a generic or a nested call inside an
 * argument does not end it early.
 */
function balanced(text, open) {
  let depth = 0;
  for (let i = open; i < text.length; i += 1) {
    const c = text[i];
    if (c === '(') depth += 1;
    else if (c === ')') {
      depth -= 1;
      if (depth === 0) return { inner: text.slice(open + 1, i), end: i + 1 };
    }
  }
  return null;
}

/**
 * Top-level comma-separated items. `<>`, `()` and `[]` all nest, so
 * `Map<String, Int>` and `foo(a, b)` each count as one. The `>` of an arrow
 * (`->`, `=>`) closes nothing.
 */
function splitTop(text) {
  const parts = [];
  let depth = 0;
  let current = '';
  for (const c of text) {
    const arrow = c === '>' && (current.endsWith('-') || current.endsWith('='));
    if (c === '<' || c === '(' || c === '[') depth += 1;
    if ((c === '>' && !arrow) || c === ')' || c === ']') depth -= 1;
    if (c === ',' && depth === 0) {
      parts.push(current);
      current = '';
    } else current += c;
  }
  parts.push(current);
  return parts.map((p) => p.trim()).filter((p) => p.length > 0);
}

/** The `#[uniffi::export] impl Host { … }` block — the methods a phone can call. */
function exportedImpl(rust) {
  const marker = rust.indexOf('#[uniffi::export]\nimpl Host {');
  if (marker < 0) return null;
  const open = rust.indexOf('{', marker);
  let depth = 0;
  for (let i = open; i < rust.length; i += 1) {
    if (rust[i] === '{') depth += 1;
    else if (rust[i] === '}') {
      depth -= 1;
      if (depth === 0) return rust.slice(open, i);
    }
  }
  return null;
}

/** camelCase name → argument count, from the Rust source. `self` does not count. */
function rustArity(rust) {
  const block = exportedImpl(rust);
  if (block === null) {
    console.error(
      'Could not find the `#[uniffi::export] impl Host` block in\n  ' +
        RUST +
        '\nThe check cannot run, which is a failure and not a pass.',
    );
    process.exit(1);
  }
  const out = new Map();
  for (const m of block.matchAll(/\bpub (?:async )?fn\s+([a-z][a-z0-9_]*)\s*\(/g)) {
    const args = balanced(block, m.index + m[0].length - 1);
    if (!args) continue;
    const params = splitTop(args.inner).filter(
      (p) => !/^(?:&(?:mut )?)?self$/.test(p.replace(/^&\s*/, '&')),
    );
    out.set(camel(m[1]), params.length);
  }
  return out;
}

/**
 * camelCase name → declared argument count, from `HostInterface` in the
 * committed bindings.
 *
 * Scoped to that one interface on purpose. The file also declares the callback
 * interfaces the host calls back INTO — `DeviceCalendarProviderInterface` among
 * them — and they share method names with Host: both have a `deleteEvent`, one
 * with a single parameter and one with three. A file-wide scan picks whichever
 * comes first and reports a disagreement that is not one.
 */
function bindingArity(bindings) {
  const start = bindings.indexOf('public interface HostInterface {');
  if (start < 0) {
    console.error(
      'Could not find `public interface HostInterface` in\n  ' +
        BINDINGS +
        '\nThe check cannot run, which is a failure and not a pass.',
    );
    process.exit(1);
  }
  const open = bindings.indexOf('{', start);
  let depth = 0;
  let end = bindings.length;
  for (let i = open; i < bindings.length; i += 1) {
    if (bindings[i] === '{') depth += 1;
    else if (bindings[i] === '}') {
      depth -= 1;
      if (depth === 0) {
        end = i;
        break;
      }
    }
  }
  const block = bindings.slice(open, end);
  const out = new Map();
  for (const m of block.matchAll(/\bfun\s+`?([A-Za-z][A-Za-z0-9_]*)`?\s*\(/g)) {
    const args = balanced(block, m.index + m[0].length - 1);
    if (!args) continue;
    out.set(m[1], splitTop(args.inner).length);
  }
  return out;
}

/** camelCase name → argument count passed, from a bridge's `host.<name>(…)` calls. */
function callArity(source, pattern) {
  const out = new Map();
  for (const m of source.matchAll(pattern)) {
    const args = balanced(source, m.index + m[0].length - 1);
    if (!args) continue;
    const count = splitTop(args.inner).length;
    // A method called twice with different shapes is itself the bug.
    const seen = out.get(m[1]);
    out.set(m[1], seen === undefined ? count : Math.max(seen, count));
  }
  return out;
}

/**
 * The `#[uniffi::export]` FREE functions in `crates/cal-ffi/src/lib.rs`, by
 * camelCase name → argument count.
 *
 * A second family, and until this was added an unwatched one. Everything above
 * follows methods called on the host OBJECT (`host.foo(…)`); a free function is
 * called by bare name on both bridges, so none of those patterns can see it.
 * `parseAttendee` had been crossing that way since it was written, and the
 * synchronous collation functions the frontends now sort with cross the same
 * way — which is precisely the boundary this file exists to watch.
 *
 * What this half checks is Rust ↔ committed bindings. That is the failure that
 * actually happens: the vendoring step refreshes the native library and NOT
 * the checked-in bindings, so Rust grows a function, Kotlin calls it, and
 * `:cal-ffi:compileReleaseKotlin` dies minutes into an EAS build.
 *
 * It does not try to find free-function CALLS in Swift. A bare call there is
 * indistinguishable from any other function call, and a pattern loose enough
 * to catch it would match half the file. Swift is covered the same way it is
 * above — through the bindings, which both platforms generate from one source.
 */
function rustFreeArity(lib) {
  const out = new Map();
  for (const m of lib.matchAll(
    /#\[uniffi::export\]\s*\n(?:\s*\/\/[^\n]*\n)*\s*pub (?:async )?fn\s+([a-z][a-z0-9_]*)\s*\(/g,
  )) {
    const args = balanced(lib, m.index + m[0].length - 1);
    if (!args) continue;
    out.set(camel(m[1]), splitTop(args.inner).length);
  }
  return out;
}

/**
 * A free function as the bindings declare it, looked up BY NAME.
 *
 * Only the backtick-quoted form counts: UniFFI writes the public API that way
 * (`` fun `parseAttendee`(…) ``) while its own plumbing is plain
 * (`internal fun setValue(…)`), so the quoting is what separates the surface
 * from the machinery. A name that appears more than once with different
 * shapes is reported rather than resolved — picking one would be inventing an
 * answer.
 */
function bindingFreeArity(bindings, name) {
  const found = new Set();
  for (const m of bindings.matchAll(
    new RegExp('\\bfun\\s+`' + name + '`\\s*\\(', 'g'),
  )) {
    const args = balanced(bindings, m.index + m[0].length - 1);
    if (args) found.add(splitTop(args.inner).length);
  }
  if (found.size === 0) return null;
  if (found.size > 1) return 'ambiguous';
  return [...found][0];
}

const rust = rustArity(readFileSync(RUST, 'utf8'));
const bindingsText = readFileSync(BINDINGS, 'utf8');
const declared = bindingArity(bindingsText);
const rustFree = rustFreeArity(readFileSync(LIB, 'utf8'));
const kotlin = callArity(
  readFileSync(KOTLIN, 'utf8'),
  /\bhost\.`?([A-Za-z][A-Za-z0-9_]*)`?\s*\(/g,
);
const swift = callArity(
  readFileSync(SWIFT, 'utf8'),
  /\bself\.host\.([A-Za-z][A-Za-z0-9_]*)\s*\(/g,
);

const problems = [];

for (const [name, count] of rustFree) {
  const declaredFree = bindingFreeArity(bindingsText, name);
  if (declaredFree === null) {
    problems.push(
      `crates/cal-ffi exports the free function ${name}(), which the committed ` +
        'bindings do not declare — the bindings are stale, and the Android ' +
        'build will fail on it',
    );
    continue;
  }
  if (declaredFree === 'ambiguous') {
    problems.push(
      `the committed bindings declare ${name}() more than once with different ` +
        'shapes, so this check cannot say which one a bridge would reach',
    );
    continue;
  }
  if (declaredFree !== count) {
    problems.push(
      `the committed bindings declare ${name}() with ${declaredFree} argument(s), ` +
        `but crates/cal-ffi declares ${count} — the bindings are stale`,
    );
  }
}

for (const [name, count] of kotlin) {
  if (!declared.has(name)) {
    problems.push(
      `the Android bridge calls host.${name}(), which the committed bindings do not declare`,
    );
    continue;
  }
  if (declared.get(name) !== count) {
    problems.push(
      `the Android bridge passes ${count} argument(s) to host.${name}(), ` +
        `but the committed bindings declare ${declared.get(name)}`,
    );
  }
}

for (const [name, count] of new Map([...kotlin, ...swift])) {
  if (!rust.has(name)) {
    problems.push(
      `a bridge calls host.${name}(), which is not in the exported Rust impl — ` +
        'either it was renamed or it is not exported over UniFFI',
    );
    continue;
  }
  if (rust.get(name) !== count) {
    const which = kotlin.get(name) === count ? 'Android' : 'iOS';
    problems.push(
      `the ${which} bridge passes ${count} argument(s) to host.${name}(), ` +
        `but crates/cal-ffi declares ${rust.get(name)}`,
    );
  }
}

for (const [name, count] of declared) {
  if (!rust.has(name)) continue;
  if (rust.get(name) !== count) {
    problems.push(
      `the committed bindings declare host.${name}() with ${count} argument(s), ` +
        `but crates/cal-ffi declares ${rust.get(name)} — the bindings are stale`,
    );
  }
}

/**
 * A source with its comments blanked out and its strings kept. Block comments
 * nest in Kotlin and Swift, not in TypeScript.
 *
 * A registration that was commented out still compiles on both platforms, and
 * a name inside a comment is nothing JavaScript can call.
 */
function withoutComments(text, { nested }) {
  const blank = (s) => s.replace(/[^\n]/g, ' ');
  let out = '';
  let i = 0;
  while (i < text.length) {
    if (text.startsWith('"""', i)) {
      const end = text.indexOf('"""', i + 3);
      const stop = end < 0 ? text.length : end + 3;
      out += text.slice(i, stop);
      i = stop;
    } else if (text[i] === '"' || text[i] === "'" || text[i] === '`') {
      const quote = text[i];
      let j = i + 1;
      while (j < text.length && text[j] !== quote && (text[j] !== '\n' || quote === '`')) {
        j += text[j] === '\\' ? 2 : 1;
      }
      out += text.slice(i, j + 1);
      i = j + 1;
    } else if (text.startsWith('//', i)) {
      const end = text.indexOf('\n', i);
      const stop = end < 0 ? text.length : end;
      out += blank(text.slice(i, stop));
      i = stop;
    } else if (text.startsWith('/*', i)) {
      let depth = 1;
      let j = i + 2;
      while (j < text.length && depth > 0) {
        if (nested && text.startsWith('/*', j)) {
          depth += 1;
          j += 2;
        } else if (text.startsWith('*/', j)) {
          depth -= 1;
          j += 2;
        } else j += 1;
      }
      out += blank(text.slice(i, j));
      i = j;
    } else {
      out += text[i];
      i += 1;
    }
  }
  return out;
}

/** The text inside the brace at `open` and its match. */
function braced(text, open) {
  let depth = 0;
  for (let i = open; i < text.length; i += 1) {
    if (text[i] === '{') depth += 1;
    else if (text[i] === '}') {
      depth -= 1;
      if (depth === 0) return text.slice(open + 1, i);
    }
  }
  return null;
}

/** A declared return type: names, generics, arrays and unions, and nothing else. */
const isReturnType = (text) =>
  /^[\w$.[\]]+(?: ?\| ?[\w$.[\]]+)*$/.test(text.replace(/<[^<>]*(?:<[^<>]*>[^<>]*)*>/g, ''));

/**
 * The functions JavaScript can call, as `CalFfiModule.ts` declares them:
 * `members` maps a name to its parameter count, whether it returns a Promise,
 * and its return type. Only the body of `declare class CalFfiModule` counts;
 * the events type above it is not callable.
 *
 * Everything above follows calls INTO Rust. A free function such as
 * `seriesShift` is registered in each native module and called there by bare
 * name, which none of those patterns see: a module that forgot to register it
 * passed this check, and TypeScript trusts the declaration, so the gap showed
 * only on the phone, as "CalFfi.seriesShift is not a function".
 *
 * A member has to be one this reads — a method, `name(args): Result;`, or a
 * property holding a function, `name: (args) => Result;`. Anything else goes
 * into `unreadable` with its text instead of being skipped: a declaration the
 * check cannot read is one it cannot hold the native modules to.
 */
function declaredFunctions(ts) {
  const source = withoutComments(ts, { nested: false });
  const marker = source.search(/\bdeclare class CalFfiModule\b/);
  const body = marker < 0 ? null : braced(source, source.indexOf('{', marker));
  if (body === null) {
    console.error(
      'Could not find the body of `declare class CalFfiModule` in\n  ' +
        TS_MODULE +
        '\nThe check cannot run, which is a failure and not a pass.',
    );
    process.exit(1);
  }
  const members = new Map();
  const unreadable = [];
  let depth = 0;
  let current = '';
  const chunks = [];
  for (const c of body) {
    if (c === '(' || c === '{' || c === '[') depth += 1;
    if (c === ')' || c === '}' || c === ']') depth -= 1;
    if (c === ';' && depth === 0) {
      chunks.push(current);
      current = '';
    } else current += c;
  }
  chunks.push(current);
  for (const chunk of chunks) {
    const text = chunk.replace(/\s+/g, ' ').trim();
    if (text === '') continue;
    const head = /^([A-Za-z_$][\w$]*) ?(<[^()]*>)? ?([(:])/.exec(text);
    let signature = null;
    if (head !== null && head[3] === '(') {
      const args = balanced(text, head[0].length - 1);
      const ret = args && /^ ?: ?(.+)$/.exec(text.slice(args.end));
      if (ret) signature = { args: args.inner, returns: ret[1] };
    } else if (head !== null && !head[2]) {
      const rest = text.slice(head[0].length).trimStart();
      const args = rest.startsWith('(') ? balanced(rest, 0) : null;
      const ret = args && /^ ?=> ?(.+)$/.exec(rest.slice(args.end));
      if (ret) signature = { args: args.inner, returns: ret[1] };
    }
    const params = signature && splitTop(signature.args);
    if (
      signature === null ||
      !isReturnType(signature.returns) ||
      params.some((p) => !/^[A-Za-z_$][\w$]* ?: ?\S/.test(p))
    ) {
      unreadable.push({ name: head?.[1] ?? null, text });
      continue;
    }
    members.set(head[1], {
      params: params.length,
      async: /^Promise</.test(signature.returns),
      returns: signature.returns,
    });
  }
  return { members, unreadable };
}

/**
 * The parameters a native closure takes from JavaScript, or `null` when this
 * cannot read them. expo's trailing `Promise` parameter is not one JavaScript
 * passes. A closure that opens straight into its body takes none.
 *
 *   Kotlin: `{ inputJson: String -> … }`, `{ -> … }`, `{ host.foo() }`
 *   Swift:  `{ (inputJson: String) -> String in … }`, `{ self.host.foo() }`
 */
function closureParams(after, language) {
  let list = null;
  if (language === 'kotlin') {
    const param = String.raw`[A-Za-z_]\w*\s*:\s*[A-Za-z_][\w.]*(?:<[^{}()]*?>)?\??`;
    const typed = new RegExp(String.raw`^\s*(?:(${param}(?:\s*,\s*${param})*)\s*)?->`).exec(after);
    if (typed) list = typed[1] ? splitTop(typed[1]) : [];
    else if (/^\s*[A-Za-z_]\w*(?:\s*,\s*[A-Za-z_]\w*)*\s*->/.test(after)) return null;
    else list = [];
  } else {
    const open = /^\s*\(/.exec(after);
    const args = open && balanced(after, open[0].length - 1);
    if (args && /^\s*(?:async\s+)?(?:throws\s+)?(?:->[^{}]*?)?\s*\bin\b/.test(after.slice(args.end))) {
      list = splitTop(args.inner);
      if (list.some((p) => !/^(?:[A-Za-z_]\w*\s+)?[A-Za-z_]\w*\s*:\s*\S/.test(p))) return null;
    } else if (/^\s*[A-Za-z_]\w*(?:\s*,\s*[A-Za-z_]\w*)*\s+in\b/.test(after)) return null;
    else list = [];
  }
  return list.filter((p) => !/:\s*Promise\s*$/.test(p)).length;
}

/**
 * name → every registration of it in one native module: `Function("…")` or
 * `AsyncFunction("…")`, and how many parameters its closure takes from
 * JavaScript (`null`: unreadable).
 *
 * Registrations are expected in the module file itself. Moving some into a
 * helper file reports them as missing, loudly, rather than passing.
 */
function registeredFunctions(source, language) {
  const code = withoutComments(source, { nested: true });
  const out = new Map();
  for (const m of code.matchAll(/\b(Async)?Function\s*\(\s*"([A-Za-z0-9_]+)"\s*\)\s*(\{)?/g)) {
    const params = m[3] ? closureParams(code.slice(m.index + m[0].length), language) : null;
    out.set(m[2], [...(out.get(m[2]) ?? []), { async: Boolean(m[1]), params }]);
  }
  return out;
}

const declaredSurface = declaredFunctions(readFileSync(TS_MODULE, 'utf8'));
const MODULE = { android: 'Android', ios: 'iOS' };
const nativeModules = new Map([
  ['android', registeredFunctions(readFileSync(KOTLIN, 'utf8'), 'kotlin')],
  ['ios', registeredFunctions(readFileSync(SWIFT, 'utf8'), 'swift')],
]);

/**
 * Functions only one platform has, each named with its reason. Their callers
 * guard on `Platform.OS`. A name listed here that turns up on both platforms, or
 * that is no longer declared, is reported too: an exception nobody needs any
 * more is how a real gap would hide behind it.
 */
const ONLY_ON = new Map([
  ['enableBackgroundRefresh', ['ios', 'the short BGAppRefreshTask wake-up exists only on iOS']],
  ['disableBackgroundRefresh', ['ios', 'cancels that iOS-only wake-up']],
  ['writeVoicePickers', ['ios', 'feeds the Siri intents, which exist only on iOS']],
]);

/** What JavaScript can call and what the native modules register, kept apart
 *  from `problems`: regenerating bindings fixes none of these. */
const surface = [];
const unreadableNames = new Set(declaredSurface.unreadable.map((u) => u.name));

for (const { text } of declaredSurface.unreadable) {
  surface.push(
    `CalFfiModule.ts has a member this check cannot read, so it cannot hold the ` +
      `native modules to it: "${text.length > 80 ? `${text.slice(0, 77)}...` : text}"`,
  );
}
for (const [platform, registered] of nativeModules) {
  const where = `the ${MODULE[platform]} module`;
  for (const name of declaredSurface.members.keys()) {
    if (ONLY_ON.has(name) && ONLY_ON.get(name)[0] !== platform) continue;
    if (!registered.has(name)) {
      surface.push(`CalFfiModule.ts declares ${name}(), which ${where} does not register`);
    }
  }
  for (const [name, found] of registered) {
    if (found.length > 1) surface.push(`${where} registers ${name}() ${found.length} times`);
    const fn = declaredSurface.members.get(name);
    if (fn === undefined) {
      if (!unreadableNames.has(name)) {
        surface.push(`${where} registers ${name}(), which CalFfiModule.ts does not declare`);
      }
      continue;
    }
    const [registration] = found;
    if (registration.async !== fn.async) {
      surface.push(
        fn.async
          ? `CalFfiModule.ts declares that ${name}() returns ${fn.returns}, but ${where} ` +
              'registers it with Function, so JavaScript gets no Promise'
          : `CalFfiModule.ts declares that ${name}() returns ${fn.returns}, but ${where} ` +
              'registers it with AsyncFunction, so JavaScript gets a Promise',
      );
    }
    if (registration.params === null) {
      surface.push(
        `this check cannot read the parameters ${where} gives ${name}() — ` +
          'write each one with its type',
      );
    } else if (registration.params !== fn.params) {
      surface.push(
        `CalFfiModule.ts declares ${name}() with ${fn.params} parameter(s), ` +
          `but ${where} takes ${registration.params}`,
      );
    }
  }
}
for (const [name, [platform]] of ONLY_ON) {
  if (!declaredSurface.members.has(name)) {
    surface.push(
      `${name}() is listed as ${MODULE[platform]} only, but CalFfiModule.ts no longer declares it`,
    );
  }
  const other = platform === 'ios' ? 'android' : 'ios';
  if (nativeModules.get(other).has(name)) {
    surface.push(
      `${name}() is listed as ${MODULE[platform]} only, but ` +
        `the ${MODULE[other]} module registers it too`,
    );
  }
}

// A parse that matched nothing would report no problems and mean nothing.
const floors = [
  ['exported Rust methods', rust.size, 100],
  ['declared bindings', declared.size, 100],
  ['Android bridge calls', kotlin.size, 100],
  ['iOS bridge calls', swift.size, 20],
  // Free functions are few by nature; the floor only has to prove the
  // parse found the family at all.
  ['exported Rust free functions', rustFree.size, 3],
  ['functions CalFfiModule.ts declares', declaredSurface.members.size, 100],
  ['Android module registrations', nativeModules.get('android').size, 100],
  ['iOS module registrations', nativeModules.get('ios').size, 100],
];
for (const [what, found, floor] of floors) {
  if (found < floor) {
    console.error(
      `Only found ${found} ${what} — the parse is wrong, so this check proves nothing.`,
    );
    process.exit(1);
  }
}

if (problems.length > 0) {
  console.error(
    `The Rust host, the committed UniFFI bindings and the native bridges ` +
      `disagree in ${problems.length} place(s):\n`,
  );
  for (const p of [...new Set(problems)].sort()) console.error(`  ${p}`);
  console.error(
    '\nIf the bindings are stale, regenerate them (see mobile/README.md):\n' +
      '  cargo build -p cal-ffi\n' +
      '  cargo run -p cal-ffi --features cli --bin uniffi-bindgen -- \\\n' +
      '    generate --library target/debug/cal_ffi.dll --language kotlin --out-dir <tmp>\n' +
      '  copy <tmp>/uniffi/cal_ffi/cal_ffi.kt over the committed one\n\n' +
      'The committed bindings and the vendored .so must come from the SAME\n' +
      'cal-ffi source, or JNA fails to resolve symbols at call time.',
  );
}

if (surface.length > 0) {
  console.error(
    `${problems.length > 0 ? '\n' : ''}CalFfiModule.ts and the native modules ` +
      `disagree in ${surface.length} place(s):\n`,
  );
  for (const p of [...new Set(surface)].sort()) console.error(`  ${p}`);
  console.error(
    '\nEvery function CalFfiModule.ts declares is registered in CalFfiModule.kt AND\n' +
      'CalFfiModule.swift: with AsyncFunction when it returns a Promise, with Function\n' +
      "otherwise, and with one closure parameter per declared parameter (expo's\n" +
      'trailing Promise parameter does not count). A function one platform lacks on\n' +
      'purpose goes into ONLY_ON in mobile/scripts/check-ffi-bridges.mjs, with its reason.',
  );
}

if (problems.length > 0 || surface.length > 0) process.exit(1);

const onlyOn = Object.entries(MODULE)
  .map(([platform, label]) => {
    const names = [...ONLY_ON].filter(([, [p]]) => p === platform).map(([n]) => n);
    return names.length > 0 ? `${label} only: ${names.sort().join(', ')}` : null;
  })
  .filter(Boolean)
  .join('; ');

console.log(
  `FFI bridges OK — the ${declaredSurface.members.size} functions CalFfiModule.ts declares ` +
    `are registered in both native modules, as the same kind and with the same ` +
    `parameter count (${onlyOn}), ` +
    `${kotlin.size} Android and ${swift.size} iOS calls agree ` +
    `with the committed bindings and with crates/cal-ffi, and its ` +
    `${rustFree.size} exported free functions ` +
    `(${[...rustFree.keys()].sort().join(', ')}) are all declared there.`,
);
