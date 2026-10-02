// Fondo animado compartido por la barra superior, los paneles laterales y la
// pestana nueva (servido como /wallpaper.js, ver src/protocol.rs). Rust nos
// manda el tema y donde cae nuestro recorte dentro del "lienzo" completo de
// la ventana (ver sync::push_wallpaper), asi las 4 superficies muestran
// fragmentos del mismo fondo y parecen uno solo continuo.
//
// <script src="/wallpaper.js" data-variant="blur"> (barra y paneles) pide la
// version chica ya difuminada: su vidrio antes difuminaba el fondo con
// backdrop-filter en cada cuadro, y a este tamano se ve igual pero cuesta
// una fraccion. Sin data-variant se usa la version 1080p.
//
// La pagina puede definir window.onWallpaperTheme(theme) para enterarse del
// tema activo (la pestana nueva resalta la pastilla correspondiente).
(function () {
  var blur = document.currentScript.getAttribute('data-variant') === 'blur';

  // Temas que son una imagen fija en vez de un video. Se animan con un zoom
  // lento por CSS (transform: lo mueve el compositor de la GPU, suave a 60fps
  // y sin nada que decodificar).
  var STILLS = ['torii-carmesi'];
  window.__isStillWallpaper = function (theme) { return STILLS.indexOf(theme) !== -1; };
  // Duracion de un acercamiento (y otro tanto el alejamiento: alternate).
  var ZOOM_S = 30;

  var box = null;      // {w, h, x, y}: ultimo recorte recibido de Rust
  var current = null;  // capa (<video>/<img>) visible
  var incoming = null; // capa del tema nuevo, cargando por debajo de la actual
  // Congelado: la pestana al frente esta reproduciendo un video o sonido (lo
  // avisa Rust, ver sync::push_media). Los videos de fondo quedan en pausa
  // sobre su ultimo cuadro: cada uno es un decodificador y una composicion
  // mas para la GPU, justo cuando el video de la pagina la necesita.
  var frozen = false;

  var style = document.createElement('style');
  style.textContent =
    '@keyframes __wallpaperZoom { from { transform: scale(1); } to { transform: scale(1.08); } }' +
    '@media (prefers-reduced-motion: reduce) { .__wallpaper { animation: none !important; } }';
  document.head.appendChild(style);

  function place(el) {
    el.style.width = box.w + 'px';
    el.style.height = box.h + 'px';
    el.style.left = (-box.x) + 'px';
    el.style.top = (-box.y) + 'px';
  }

  function isVideo(el) {
    return el.tagName === 'VIDEO';
  }

  // Cada superficie es un WebView (proceso) distinto con su propio fondo, y
  // cada uno arrancaria cuando termina de cargar: la barra mostraria otro
  // instante que la pagina de abajo y el "fondo unico" se cortaria en el
  // borde. Todas comparten el reloj del sistema, asi que cada una se
  // posiciona en (ahora % duracion) y quedan alineadas sin hablarse.
  function clockTime(duration) {
    return (Date.now() / 1000) % duration;
  }

  function drift(v) {
    var d = v.currentTime - clockTime(v.duration);
    // El video da la vuelta (loop): 0.1 y duracion-0.1 estan a 0.2s, no a toda la duracion.
    if (d > v.duration / 2) d -= v.duration;
    if (d < -v.duration / 2) d += v.duration;
    return d;
  }

  function discard(el) {
    if (!el) return;
    if (isVideo(el)) { el.removeAttribute('src'); el.load(); }
    el.remove();
  }

  // Recien cuando el tema nuevo ya tiene algo en pantalla sacamos el
  // anterior: cambiar de tema no deja un hueco oscuro mientras carga.
  function reveal(el) {
    if (incoming !== el) return;
    discard(current);
    current = el;
    incoming = null;
  }

  function makeVideo(src) {
    var v = document.createElement('video');
    v.muted = true; v.loop = true; v.playsInline = true; v.preload = 'auto';
    // opacity:.9999 (no 1) es a proposito: con opacidad exactamente 1,
    // Chromium promueve el <video> a un "overlay" de hardware compuesto
    // por DirectComposition por fuera de la cadena normal de dibujado —
    // y esa via no se mezcla con una ventana transparente/con blur como
    // esta (ver with_transparent+with_blur en main.rs), asi que el video
    // decodifica (se ve en el estado interno) pero nunca aparece en
    // pantalla. Bajar la opacidad un poco fuerza la via de composicion
    // normal, que si respeta la transparencia de la ventana.
    v.style.opacity = '.9999';
    v.addEventListener('loadedmetadata', function () {
      v.currentTime = clockTime(v.duration);
      // Aunque este congelado hace falta un cuadro en pantalla: arranca, y
      // apenas suena el primer cuadro se pausa (ver 'playing').
      if (!document.hidden) v.play().catch(function () {});
    }, { once: true });
    v.addEventListener('playing', function () {
      reveal(v);
      if (frozen) v.pause();
    }, { once: true });
    v.src = src;
    return v;
  }

  function makeImage(src) {
    var img = document.createElement('img');
    img.decoding = 'async';
    img.style.animation = '__wallpaperZoom ' + ZOOM_S + 's ease-in-out infinite alternate';
    // Delay negativo = arrancar la animacion ya avanzada: todas las
    // superficies quedan en el mismo punto del zoom (mismo reloj del sistema).
    img.style.animationDelay = (-clockTime(2 * ZOOM_S)) + 's';
    img.style.willChange = 'transform';
    img.src = src;
    // decode(): mostrarla recien ya decodificada, sin un cuadro trabado.
    // (Puede fallar sin que la imagen este rota; entonces vale con que cargo.)
    img.decode().catch(function () {}).then(function () {
      if (img.naturalWidth) reveal(img);
    });
    return img;
  }

  function makeLayer(theme) {
    var still = STILLS.indexOf(theme) !== -1;
    // Ruta relativa (no 'app://localhost/...'): en Windows, WebView2 sirve
    // el protocolo personalizado internamente como 'http://app.localhost/',
    // asi que una URL absoluta con el esquema literal 'app://' nunca llega
    // a dispararse como peticion de red desde el propio DOM (aunque la
    // navegacion inicial de la pestana si acepta ese esquema). Una ruta
    // relativa resuelve siempre contra el origen real de la pagina, sea
    // cual sea, y por eso es la unica forma que funciona en toda plataforma.
    var src = '/wallpapers/' + theme + (blur ? '.blur' : '') + (still ? '.jpg' : '.mp4');
    var el = still ? makeImage(src) : makeVideo(src);
    el.className = '__wallpaper';
    el.setAttribute('data-theme', theme);
    el.style.position = 'fixed';
    el.style.zIndex = '-1';
    el.style.objectFit = 'cover';
    el.style.pointerEvents = 'none';
    // El zoom de las imagenes es respecto del centro de la VENTANA (la capa
    // mide toda la ventana), asi coincide en las 4 superficies.
    el.style.transformOrigin = '50% 50%';
    place(el);
    // Antes de la actual en el DOM: a igual z-index se pinta debajo de ella.
    document.body.insertBefore(el, document.body.firstChild);
    return el;
  }

  function setFrozen(value) {
    value = !!value;
    if (value === frozen) return;
    frozen = value;
    [current, incoming].forEach(function (v) {
      if (!v || !isVideo(v)) return;
      if (frozen) {
        v.pause();
      } else if (!document.hidden && v.duration > 0) {
        // Se retoma donde va el reloj comun, como las demas superficies.
        v.currentTime = clockTime(v.duration);
        v.play().catch(function () {});
      }
    });
  }
  window.__setWallpaperFrozen = setFrozen;

  window.__applyWallpaper = function (theme, winW, winH, offX, offY, freeze) {
    if (freeze !== undefined) setFrozen(freeze);
    box = { w: winW, h: winH, x: offX, y: offY };
    if (current) place(current);
    if (incoming) place(incoming);

    var shown = incoming || current;
    if (!shown || shown.getAttribute('data-theme') !== theme) {
      discard(incoming);
      incoming = makeLayer(theme);
    }
    if (window.onWallpaperTheme) window.onWallpaperTheme(theme);
  };

  // Correccion de deriva de los videos: cada uno lleva su propio reloj y el
  // salto del loop cuesta unos ms, asi que con el tiempo se separan del
  // reloj comun. Se corrige ajustando apenas la velocidad (invisible) en vez
  // de saltar (seek), que se nota como un tiron; solo si se fue muy lejos se
  // salta. (Las imagenes no derivan: su animacion CSS ya usa el reloj comun.)
  setInterval(function () {
    var v = current;
    if (!v || !isVideo(v) || v.paused || !(v.duration > 0)) return;
    var d = drift(v);
    if (Math.abs(d) > 1) {
      v.currentTime = clockTime(v.duration);
      v.playbackRate = 1;
    } else if (Math.abs(d) > 0.03) {
      v.playbackRate = d > 0 ? 0.98 : 1.02;
    } else {
      v.playbackRate = 1;
    }
  }, 500);

  // Una pestana en segundo plano (Rust la oculta, ver layout::relayout) no
  // tiene por que seguir decodificando video.
  document.addEventListener('visibilitychange', function () {
    [current, incoming].forEach(function (v) {
      if (!v || !isVideo(v)) return;
      if (document.hidden) {
        v.pause();
      } else if (v.duration > 0 && !frozen) {
        v.currentTime = clockTime(v.duration);
        v.play().catch(function () {});
      }
    });
  });
})();
