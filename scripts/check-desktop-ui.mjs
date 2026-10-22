/** Inspect an explicitly instrumented local WebView2 without changing the app.
 * Usage: node scripts/check-desktop-ui.mjs --port 9223 [--offline] [--reload] [--security] [--close]
 * Offline emulation applies only to this debugger target, never system networking.
 */
const args = process.argv.slice(2);
const portIndex = args.indexOf('--port');
const port = portIndex < 0 ? 9223 : Number(args[portIndex + 1]);
if (!Number.isInteger(port) || port < 1 || port > 65535) {
  throw new Error('--port requires an integer from 1 through 65535');
}
const targets = await (await fetch(`http://127.0.0.1:${port}/json/list`)).json();
const target = targets.find(item => item.type === 'page');
if (!target) throw new Error('No desktop WebView target at the supplied port');
const socket = new WebSocket(target.webSocketDebuggerUrl);
await new Promise((resolve, reject) => {
  socket.addEventListener('open', resolve, { once: true });
  socket.addEventListener('error', reject, { once: true });
});
let next = 1;
const pending = new Map();
socket.addEventListener('message', event => {
  const reply = JSON.parse(event.data);
  const callback = pending.get(reply.id);
  if (!callback) return;
  pending.delete(reply.id);
  clearTimeout(callback.timeout);
  if (reply.error) callback.reject(new Error(reply.error.message));
  else callback.resolve(reply.result);
});
function send(method, params = {}) {
  return new Promise((resolve, reject) => {
    const id = next++;
    const timeout = setTimeout(() => { pending.delete(id); reject(new Error(`${method} timed out`)); }, 30000);
    pending.set(id, { resolve, reject, timeout });
    socket.send(JSON.stringify({ id, method, params }));
  });
}
async function evaluate(expression) {
  const result = await send('Runtime.evaluate', { expression, returnByValue: true, awaitPromise: true, allowUnsafeEvalBlockedByCSP: false });
  if (result.exceptionDetails) throw new Error(result.exceptionDetails.text);
  return result.result.value;
}
try {
  await send('Runtime.enable');
  if (args.includes('--offline')) {
    await send('Network.enable');
    await send('Network.emulateNetworkConditions', { offline: true, latency: 0, downloadThroughput: 0, uploadThroughput: 0 });
  }
  if (args.includes('--reload')) {
    await send('Page.reload', { ignoreCache: true });
    const until = Date.now() + 30000;
    while (Date.now() < until) {
      if (await evaluate(`document.readyState === 'complete' && !!document.querySelector('h1')`)) break;
      await new Promise(resolve => setTimeout(resolve, 100));
    }
  }
  await send('HeapProfiler.enable');
  await send('HeapProfiler.collectGarbage');
  const heap = await send('Runtime.getHeapUsage');
  const ui = await evaluate(`(() => {
    const viewport = document.querySelector('.viewport');
    const files = [...document.querySelectorAll('button.file')];
    const region = [...document.querySelectorAll('[aria-label]')].find(item => item.getAttribute('aria-label') === 'Files');
    return { url: location.href, title: document.title, nativeBridge: !!window.__TAURI__,
      heading: document.querySelector('h1')?.innerText, renderedRows: files.length,
      firstFile: files[0]?.innerText, lastFile: files.at(-1)?.innerText,
      collection: region?.querySelector('p')?.innerText,
      scrollHeight: viewport?.scrollHeight, scrollTop: viewport?.scrollTop,
      renderedImages: document.images.length,
      loadedImages: [...document.images].filter(image => image.complete && image.naturalWidth > 0).length };
  })()`);
  let security;
  if (args.includes('--security')) {
    security = await evaluate(`(async () => {
      let dynamicCodeBlocked = false;
      try { new Function('return 1')(); } catch (error) { dynamicCodeBlocked = error instanceof EvalError; }
      const remoteScript = await new Promise(resolve => {
        const url = 'https://example.com/revenant-acceptance-csp.js';
        const script = document.createElement('script');
        const timeout = setTimeout(() => finish(false), 3000);
        function finish(blocked) {
          clearTimeout(timeout); document.removeEventListener('securitypolicyviolation', violation);
          script.remove(); resolve(blocked);
        }
        function violation(event) {
          if (event.blockedURI === url && event.effectiveDirective.startsWith('script-src')) finish(true);
        }
        document.addEventListener('securitypolicyviolation', violation);
        script.src = url; document.head.appendChild(script);
      });
      return { dynamicCodeBlocked, remoteScriptBlocked: remoteScript,
        policy: document.querySelector('meta[http-equiv="Content-Security-Policy"]')?.content ?? null };
    })()`);
    if (!security.dynamicCodeBlocked || !security.remoteScriptBlocked) {
      throw new Error('Packaged CSP acceptance failed: ' + JSON.stringify(security));
    }
  }
  console.log(JSON.stringify({ offline: args.includes('--offline'), heap, ui, security }, null, 2));
  if (args.includes('--close')) {
    // Destruction can tear down the debugger before its invoke promise replies.
    await evaluate(`(() => { setTimeout(() => window.__TAURI__.core.invoke('plugin:window|destroy', { label: 'main' }), 0); return true; })()`);
  }
} finally { socket.close(); }
