// Se inyecta en el popup de una extension (el que se abre al tocar su icono
// en la barra, ver src/popup.rs). Rust reemplaza __TARGET_URL__ por la URL
// de la pestana que el usuario estaba mirando.
//
// 1) "La pestana actual". En WebView2 cada WebView es una ventana aparte con
//    una sola pestana, asi que chrome.tabs.query({active: true,
//    currentWindow: true}) desde el popup devolvia EL PROPIO POPUP, y la
//    extension decia cosas como "no es un sitio web". Ese pedido (y sus
//    variantes: lastFocusedWindow, highlighted) se contesta con la pestana
//    real: la que tiene esa URL y esta visible (su ventana tiene ancho > 0;
//    las pestanas en segundo plano miden 0x0, ver layout::relayout).
// 2) Tamano. Como en Chrome, el popup mide lo que mide su contenido: se le
//    avisa a Rust (popup_size) cada vez que cambia.
// 3) window.close() (lo usan muchas extensiones al terminar una accion)
//    cierra el popup.
(function () {
  if (location.protocol !== 'chrome-extension:') return;
  var TARGET = __TARGET_URL__;

  function post(msg) {
    try { window.ipc.postMessage(JSON.stringify(msg)); } catch (e) {}
  }

  window.close = function () { post({ cmd: 'popup_close' }); };

  // --- Tamano ---
  var raf = 0;
  function report() {
    raf = 0;
    var html = document.documentElement, body = document.body;
    if (!body) return;
    // Ancho "natural" del contenido (como calcula Chrome el del popup):
    // se mide con el documento en max-content y se vuelve a como estaba.
    var prev = html.style.width;
    html.style.width = 'max-content';
    var w = Math.ceil(Math.max(html.getBoundingClientRect().width, body.scrollWidth));
    html.style.width = prev;
    var h = Math.ceil(Math.max(html.scrollHeight, body.scrollHeight, body.getBoundingClientRect().height));
    post({ cmd: 'popup_size', w: w, h: h });
  }
  // setTimeout y no requestAnimationFrame: el popup nace oculto hasta saber
  // su tamano, y en un WebView oculto requestAnimationFrame nunca corre.
  function schedule() { if (!raf) raf = setTimeout(report, 16); }
  function start() {
    schedule();
    try { new ResizeObserver(schedule).observe(document.body); } catch (e) {}
    try { new MutationObserver(schedule).observe(document.body, { childList: true, subtree: true, attributes: true }); } catch (e) {}
    setTimeout(schedule, 150);
    setTimeout(schedule, 600);
  }
  if (document.readyState === 'loading') document.addEventListener('DOMContentLoaded', start);
  else start();

  // --- "La pestana actual" ---
  if (!window.chrome || !chrome.tabs || !chrome.tabs.query) return;
  var query = chrome.tabs.query.bind(chrome.tabs);
  var getAllWindows = chrome.windows && chrome.windows.getAll ? chrome.windows.getAll.bind(chrome.windows) : null;
  // Forma con callback: sirve igual en extensiones MV2 y MV3.
  function queryP(q) { return new Promise(function (res) { query(q, function (t) { res(t || []); }); }); }

  function targetTab() {
    return queryP({}).then(function (all) {
      var candidates = all.filter(function (t) { return t.url === TARGET || t.pendingUrl === TARGET; });
      if (candidates.length <= 1 || !getAllWindows) return candidates[0] || null;
      return new Promise(function (res) {
        getAllWindows({}, function (wins) {
          var visible = {};
          (wins || []).forEach(function (w) { if (w.width > 0) visible[w.id] = true; });
          res(candidates.filter(function (t) { return visible[t.windowId]; })[0] || candidates[0]);
        });
      });
    });
  }

  chrome.tabs.query = function (q, cb) {
    var current = q && (q.active === true || q.currentWindow === true || q.lastFocusedWindow === true || q.highlighted === true);
    var p = current
      ? targetTab().then(function (t) { return t ? [t] : []; })
      : queryP(q || {});
    if (typeof cb === 'function') { p.then(cb); return undefined; }
    return p;
  };
})();
