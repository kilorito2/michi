// Se inyecta en TODAS las paginas de contenido (todas las pestanas), en
// todas las navegaciones, antes de que la pagina cargue. Hace dos cosas:
//
// 1. Avisa a Rust cuando la pagina tiene un video reproduciendose (ver
//    ipc::media_message): mientras tanto la interfaz congela su fondo
//    animado, que le quita GPU al video. Solo manda "michi:media:1" o
//    "michi:media:0", nunca nada de la pagina.
// 2. En la portada de Google, ver mas abajo.
(function () {
  if (!window.ipc || typeof window.ipc.postMessage !== 'function') return;
  var MIN_AREA = 160 * 90; // un videito de avatar no cuenta
  var STOP_DELAY = 1500;   // TikTok/YouTube pausan uno y arrancan otro: no parpadear
  var playing = new Set();
  var sent = false;
  var timer = 0;

  function report(value) {
    if (value === sent) return;
    sent = value;
    try { window.ipc.postMessage('michi:media:' + (value ? '1' : '0')); } catch (e) {}
  }

  function update() {
    playing.forEach(function (v) {
      if (!v.isConnected || v.paused || v.ended) playing.delete(v);
    });
    if (playing.size) {
      clearTimeout(timer);
      timer = 0;
      report(true);
    } else if (sent && !timer) {
      timer = setTimeout(function () {
        timer = 0;
        update();
        if (!playing.size) report(false);
      }, STOP_DELAY);
    }
  }

  // Los eventos de media no burbujean, pero la fase de captura en window si
  // los ve (todos los <video> de la pagina, aunque los cree el sitio despues).
  window.addEventListener('playing', function (e) {
    var v = e.target;
    if (!v || v.tagName !== 'VIDEO' || v.offsetWidth * v.offsetHeight < MIN_AREA) return;
    playing.add(v);
    update();
  }, true);
  ['pause', 'ended', 'emptied', 'abort'].forEach(function (name) {
    window.addEventListener(name, function (e) {
      if (!playing.delete(e.target)) return;
      update();
    }, true);
  });
})();

// Portada de Google: la deja en un tono oscuro que combina con el resto de la
// interfaz glassmorphism y mostrar unicamente la barra de busqueda (sin logo, sin
// barra superior, sin botones, sin pie de pagina).
//
// Nota: se probo dejar el fondo realmente transparente (viendose el
// escritorio difuminado detras, como en el resto de la app), pero
// WebView2/Chromium reservaba una region blanca opaca en esa pagina en
// particular que no respondia a CSS ni a with_background_color (un
// problema de composicion GPU ajeno al DOM, no algo que se pueda arreglar
// desde aqui) - por eso se opta por un tono oscuro solido en vez de una
// transparencia que a veces se rompe.
(function () {
  function isGoogleHome() {
    var host = location.hostname.replace(/^www\./, '');
    var isGoogleDomain = /^google\.[a-z.]{2,8}$/i.test(host);
    var isHomePath = location.pathname === '/' || location.pathname === '/webhp' || location.pathname === '';
    return isGoogleDomain && isHomePath;
  }

  if (!isGoogleHome()) return;

  var DARK_BG = '#141225';

  var injected = false;
  function inject() {
    if (injected) return;
    injected = true;
    var style = document.createElement('style');
    style.setAttribute('data-injected-by', 'navegador');
    style.textContent = [
      'html, body { background: ' + DARK_BG + ' !important; }',
      // Barra superior (Gmail/Imagenes/apps/Acceder), logo y botones de abajo.
      '#gb, svg[aria-label="Google"], input[type="submit"] { display: none !important; }',
      // Indicador de pais ("Argentina", etc.) y pie de pagina (Sobre Google,
      // Privacidad, Condiciones...): el pie se ubica por los enlaces que
      // contiene en vez de por clases (Google las cambia seguido), asi que
      // es mas resistente a futuros rediseños.
      '.O3yKUb { display: none !important; }',
      'div:has(> a[href*="about.google"]) { display: none !important; }',
      'div:has(> a[href*="policies.google.com"]) { display: none !important; }',
    ].join('\n');
    (document.head || document.documentElement).appendChild(style);
  }

  // Ademas de la hoja de estilos de arriba, recorremos todos los elementos
  // de la pagina y les damos el mismo fondo oscuro (salvo al propio
  // formulario de busqueda, que debe conservar su pastilla): Google arma
  // la portada con varios contenedores anidados y alguno de ellos pinta
  // blanco por su cuenta, con mas prioridad que un <style> nuestro.
  function forceDarkBackground() {
    if (!document.body) return;
    document.documentElement.style.setProperty('background', DARK_BG, 'important');
    document.body.style.setProperty('background', DARK_BG, 'important');

    var keep = document.querySelector('form[role="search"]');
    var all = document.body.querySelectorAll('*');
    for (var i = 0; i < all.length; i++) {
      var el = all[i];
      if (keep && (el === keep || keep.contains(el))) continue;
      var bg = getComputedStyle(el).backgroundColor;
      if (bg && bg !== 'rgba(0, 0, 0, 0)' && bg !== 'transparent') {
        el.style.setProperty('background-color', DARK_BG, 'important');
        el.style.setProperty('background-image', 'none', 'important');
      }
    }
  }

  var observing = false;
  function watchForChanges() {
    if (observing || !document.body) return;
    observing = true;
    // Google sigue modificando el DOM despues de la carga inicial (temas,
    // pruebas A/B, contenido diferido); reaplicamos cada vez que agrega o
    // quita nodos. Solo miramos childList/subtree (no atributos) para no
    // reaccionar a los propios cambios de estilo que hacemos nosotros.
    new MutationObserver(forceDarkBackground)
      .observe(document.body, { childList: true, subtree: true });
  }

  function run() {
    inject();
    forceDarkBackground();
    watchForChanges();
  }

  if (document.documentElement) run();
  document.addEventListener('DOMContentLoaded', run);
  // Reintento tardio por si algun script de la propia pagina reescribe el
  // fondo despues de cargar (p. ej. al resolver el tema claro/oscuro).
  setTimeout(run, 300);
  setTimeout(run, 1000);
  setTimeout(run, 2500);
})();
