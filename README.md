<div align="center">

<img src="docs/img/banner.png" alt="Michi — un navegador para Windows hecho en Rust" width="100%">

<br>

![Windows 10 | 11](https://img.shields.io/badge/Windows-10%20%7C%2011-0078D6?style=for-the-badge&logo=windows11&logoColor=white)
![Rust](https://img.shields.io/badge/Rust-2021-CE422B?style=for-the-badge&logo=rust&logoColor=white)
![WebView2](https://img.shields.io/badge/Motor-WebView2%20(Chromium)-0A7CFF?style=for-the-badge&logo=microsoftedge&logoColor=white)
![Versión](https://img.shields.io/badge/versi%C3%B3n-0.1.8-ff7a1a?style=for-the-badge)
![Licencia MIT](https://img.shields.io/badge/licencia-MIT-8b7bff?style=for-the-badge)
[![Compilar y probar](https://img.shields.io/github/actions/workflow/status/kilorito2/michi/compilar.yml?branch=main&style=for-the-badge&label=compila)](https://github.com/kilorito2/michi/actions/workflows/compilar.yml)

### [Descargar el instalador](https://github.com/kilorito2/michi/releases/latest)

**[Funciones](#funciones)** ·
**[Seguridad](#seguridad)** ·
**[Instalador](#instalador)** ·
**[Compilar](#compilar)** ·
**[Arquitectura](#arquitectura)**

</div>

<br>

<p align="center">
  <img src="docs/img/portada.jpg" alt="La pestaña nueva de Michi, con el fondo animado de una ciudad de noche" width="100%">
</p>

**Michi** es un navegador de escritorio para Windows con una idea distinta de
cómo se reparte la pantalla: **no hay barra de pestañas arriba**. La página ocupa
todo el espacio, y las pestañas aparecen deslizándose desde el borde derecho al
acercar el cursor; las descargas, el historial y los marcadores, desde el borde
izquierdo. Arriba queda solo una barra delgada de vidrio, con un **fondo animado**
que recorre toda la interfaz como si fuera una sola imagen.

Por dentro, cada pestaña es un motor **Chromium real** (WebView2, el mismo de
Microsoft Edge): cualquier sitio funciona como en un navegador normal. Encima de
eso, Michi suma su propia capa de **seguridad**: HTTPS primero, SmartScreen,
bloqueo de ventanas emergentes, extensiones con firma verificada y mucho más.

<br>

<a id="indice"></a>

## <img src="docs/icons/indice.svg" width="26" height="26" alt=""> Índice

- [Funciones](#funciones)
- [Seguridad](#seguridad)
- [Privacidad: qué sale de tu PC](#privacidad)
- [Instalador](#instalador)
- [Compilar desde el código](#compilar)
- [Arquitectura](#arquitectura)
- [Decisiones de diseño](#decisiones)
- [Limitaciones conocidas](#limitaciones)
- [Hoja de ruta](#hoja-de-ruta)

<br>

<a id="funciones"></a>

## <img src="docs/icons/funciones.svg" width="26" height="26" alt=""> Funciones

### <img src="docs/icons/pestanas.svg" width="22" height="22" alt=""> Pestañas al costado

Las pestañas viven en un panel de vidrio que se desliza desde el **borde
derecho** al acercar el cursor, y se esconde solo al alejarlo. La página nunca
cambia de tamaño: el panel pasa **por encima**, así nada salta ni se reacomoda.
Cada pestaña muestra el **logo del sitio** (su favicon real, tomado del propio
motor, sin pedirlo a ningún servicio externo); hasta que carga, un puntito.

- **Arrástralas** para cambiarlas de orden.
- **Fíjalas** (clic derecho › *Fijar pestaña*): quedan arriba de todo, solo con
  su logo, no se cierran con *Cerrar las demás* y vuelven fijadas al restaurar
  la sesión.
- Las que suenan muestran un **altavoz**; un clic las **silencia** (y otro les
  devuelve el sonido).
- **Clic derecho** sobre una pestaña: fijar, silenciar, volver a cargar,
  duplicar, cerrar las demás y cerrar. **Clic con la rueda**: la cierra.
- **Grupos de pestañas**, como en Chrome: clic derecho › *Agregar a un grupo
  nuevo* (o a uno que ya existe). El grupo tiene nombre y color, se **pliega**
  con un clic en su encabezado para que ocupe una sola fila, y desde su clic
  derecho se cambia el nombre o el color, se abre una pestaña nueva adentro, se
  desagrupa o se cierra entero. Arrastrar una pestaña entre las de un grupo la
  mete en él. Los grupos vuelven al restaurar la sesión.

<p align="center">
  <img src="docs/img/pestanas-panel.gif" alt="Se abre el panel de pestañas, se arrastra una pestaña hasta arriba, se fija otra con el clic derecho y se ve el altavoz tachado de una pestaña silenciada" width="100%">
</p>

<p align="center">
  <img src="docs/img/grupos.gif" alt="Con el clic derecho se crea el grupo Programación con dos pestañas, se le cambia el color a verde y se pliega" width="100%">
</p>

### <img src="docs/icons/sugerencias.svg" width="22" height="22" alt=""> Barra de direcciones con sugerencias

Como en Firefox:

- Al entrar a la barra aparecen tus **sitios más visitados**, cada uno con su logo.
- Al escribir, se **completa en línea** el sitio que más visitas que empieza
  así: `y` → `youtube.com/`, con lo agregado seleccionado (seguir escribiendo
  lo reemplaza).
- Debajo, las **sugerencias de tu buscador** y lo que coincide de tu
  **historial**, tus **marcadores** y tus **pestañas abiertas** (con *Cambiar a
  la pestaña*), sin importar mayúsculas ni acentos.
- <kbd>↑</kbd> <kbd>↓</kbd> recorren la lista, <kbd>Enter</kbd> abre lo
  elegido y <kbd>Esc</kbd> cierra.

> Para las sugerencias de búsqueda, lo que escribes se envía al buscador
> elegido (como en Firefox o Chrome, pero sin cookies). Nunca se envía algo con
> forma de dirección: un sitio, `localhost` o una IP. Se apaga en **Ajustes ›
> General**, y el historial, los marcadores y las pestañas se siguen sugiriendo
> sin enviar nada. Los logos guardados de los sitios se borran junto con el
> historial.

### <img src="docs/icons/ventanas.svg" width="22" height="22" alt=""> Menú del clic derecho

El menú de las páginas es el de Chromium (copiar, pegar, guardar imagen,
inspeccionar...) con lo que trae cualquier navegador y WebView2 no:

| Sobre | Opciones que agrega Michi |
| --- | --- |
| Un enlace | **Abrir enlace en una pestaña nueva** (atrás, junto a la actual, como en Chrome) y **en una pestaña de incógnito** |
| Una imagen | **Abrir imagen en una pestaña nueva** |
| Texto seleccionado | **Buscar "…" en Google** (o en el buscador que elijas en Ajustes) |

Las que se abren atrás desde una misma pestaña quedan en orden, una después de
otra. *Abrir vínculo en una ventana nueva* de WebView2 ya no aparece: en Michi
no hay ventanas, hay pestañas.

### <img src="docs/icons/descargas.svg" width="22" height="22" alt=""> Descargas, historial y marcadores

Desde el **borde izquierdo** (o con <kbd>Ctrl</kbd>+<kbd>H</kbd>): las descargas
con su progreso, pausa, reanudar y cancelar; el historial; y los marcadores. Los
archivos peligrosos esperan a que decidas **Conservar** o **Descartar**.

- **Historial con buscador**: busca en el título y la dirección, sin importar
  mayúsculas ni acentos. Se agrupa por día (*Hoy*, *Ayer*, *lunes 12 de
  octubre*...) con la hora de cada visita, y cada página aparece una sola vez.
  La ✕ de una fila la saca del historial; *Borrar* pide confirmación.
- **Marcadores en carpetas**: *Nueva carpeta*, y el lápiz de cada marcador
  cambia su nombre, su dirección o la carpeta (o crea una en el momento). Las
  carpetas se abren y cierran con un clic, se renombran, y al quitar una sus
  marcadores no se borran: quedan sueltos. Los que se importan de otro
  navegador llegan juntos en una carpeta *De Firefox*, *De Google Chrome*...

<p align="center">
  <img src="docs/img/biblioteca.gif" alt="Se abre el panel izquierdo, se busca gato en el historial y se mueve un marcador a una carpeta nueva llamada Gatos" width="100%">
</p>

El panel no se cierra mientras escribes en él (el buscador, un marcador), aunque
el cursor se aleje: se cierra con un clic fuera.

### <img src="docs/icons/fondos.svg" width="22" height="22" alt=""> Fondos animados

Cuatro fondos animados (videos a 60 fps, o una imagen con un zoom lento) que se
ven detrás de toda la interfaz: la barra, los paneles, el menú y la pestaña nueva
muestran cada uno **su pedazo** del mismo fondo, sincronizados con el reloj del
sistema, así se ven como una sola imagen continua.

| Fondo | Tipo |
| --- | --- |
| <img src="docs/icons/lluvia.svg" width="20" height="20" alt=""> **Lluvia nocturna** | Video |
| <img src="docs/icons/torii.svg" width="20" height="20" alt=""> **Torii carmesí** | Imagen con zoom lento |
| <img src="docs/icons/luna.svg" width="20" height="20" alt=""> **Pétalos y luna** | Video |
| <img src="docs/icons/olas.svg" width="20" height="20" alt=""> **Wuthering Waves** | Video |

Se cambian desde la pestaña nueva o en **Ajustes › Apariencia**.

Mientras la pestaña que estás viendo reproduce un video o suena algo (TikTok,
YouTube...), los fondos de la barra y los paneles **se congelan** en su último
cuadro y se reanudan solos al terminar: cada fondo es un decodificador de video
y una composición más, y la GPU hace falta para el video de la página.

### <img src="docs/icons/incognito.svg" width="22" height="22" alt=""> Pestañas de incógnito

<kbd>Ctrl</kbd> + <kbd>Mayús</kbd> + <kbd>N</kbd>, o **⋮ › Pestaña de incógnito**
(y un botón en **Ajustes › Privacidad**). Es una pestaña más del panel, pero con
un **perfil aparte que vive solo en memoria** (el modo InPrivate de WebView2):
sus cookies, caché, almacenamiento de sitios y permisos no se mezclan con los de
las pestañas normales, y desaparecen cuando se cierran todas las de incógnito.

- La barra se tiñe de violeta y muestra una etiqueta **Incógnito**; en el panel
  de pestañas llevan un fantasma.
- No dejan **historial**, ni logos de sitios para las sugerencias, ni se guardan
  en la **sesión** (no se restauran al iniciar) ni se pueden **reabrir** con
  <kbd>Ctrl</kbd> + <kbd>Mayús</kbd> + <kbd>T</kbd>.
- Lo que se abre desde una pestaña de incógnito (enlaces, ventanas emergentes) también es de
  incógnito. Sus pestañas solo se sugieren en la barra de direcciones desde otra de incógnito.
- Las descargas y los marcadores que guardes **se conservan**, y tu proveedor de
  internet y los sitios que visitas igual ven lo que haces: incógnito no es anonimato.

### <img src="docs/icons/rendimiento.svg" width="22" height="22" alt=""> Modos de rendimiento

En **Ajustes › Rendimiento**; se aplican en el momento, sin reiniciar.

| Modo | Qué hace |
| --- | --- |
| **Normal** | Todo como siempre. |
| **Optimizado** | El fondo animado se **pausa cuando Michi no está en primer plano** (otra ventana al frente, o minimizado). Las pestañas de atrás piden usar menos memoria y, a los **10 minutos** sin usarlas, se **duermen**. |
| **Super optimizado** | El fondo pasa a ser una **imagen fija** (un cuadro del video: no se decodifica ningún video), la interfaz **no anima nada** (sin transiciones ni desenfoque, y los paneles se abren de golpe). Las pestañas de atrás usan la menor memoria posible y se duermen a los **2 minutos**. |

Una pestaña dormida (`TrySuspend` de WebView2) no ejecuta su página: se pausan sus
temporizadores y conexiones, así que un chat o un correo abierto de fondo no avisa
de lo nuevo hasta que vuelvas a ella (al volver se reanuda sola). No se duermen las
pestañas que reproducen sonido o video, las que están cargando, las páginas propias
de Michi ni la que está en pantalla completa. Los modos viven en `src/perf.rs` y
`ui/wallpaper.js`.

### <img src="docs/icons/importar.svg" width="22" height="22" alt=""> Importar de otro navegador

En **Ajustes › Importar**, Michi detecta los navegadores instalados en la PC y
trae, del perfil que elijas, tus **marcadores**, el **historial**, las
**sesiones iniciadas** (cookies) y las **pestañas abiertas**. Se agregan a lo que
ya tienes, sin reemplazar nada.

| Navegador | Qué trae |
| --- | --- |
| **Firefox** (y LibreWolf, Waterfox, Zen, Floorp) | Marcadores, historial, sesiones y pestañas abiertas. Firefox guarda sus cookies sin cifrar, así que las sesiones se copian tal cual: sigues conectado en los sitios. |
| **Chrome, Edge, Brave, Opera, Vivaldi** | Marcadores e historial. Estos navegadores cifran sus cookies con Windows (DPAPI), así que las sesiones **no** se importan: vas a tener que iniciar sesión de nuevo. |

> Para que las sesiones se copien bien, cierra antes el otro navegador. Las bases
> se leen sobre una copia temporal, así que no se toca ni se modifica nada del
> navegador de origen.

### <img src="docs/icons/extensiones.svg" width="22" height="22" alt=""> Extensiones de Chrome y Edge

Con la página de una extensión abierta en **Chrome Web Store** o **Complementos
de Edge**, la barra ofrece **Agregar a Michi**. El propio botón de la tienda
("Agregar a Chrome", "Obtener") también funciona: Michi no guarda el paquete
en Descargas, sino que lo pide directamente a la tienda oficial y lo instala.
También se instalan desde un `.crx`, un `.zip` o una carpeta.

Antes de instalar, Michi **verifica la firma digital** del paquete y muestra lo
que la extensión va a poder hacer:

> **Agregar "uBlock Origin Lite"?**
>
> Origen: Chrome Web Store, firma verificada.
>
> Va a poder:
> - Leer y cambiar todos tus datos en todos los sitios web
> - Bloquear o cambiar contenido de las páginas

Las que tienen ventana propia aparecen como un icono en la barra superior; su
popup se abre flotando debajo, como en Chrome, y actúa sobre la pestaña que
estás viendo.

### <img src="docs/icons/usuario.svg" width="22" height="22" alt=""> Contraseñas

Michi tiene su propio gestor de contraseñas (WebView2 sabe guardarlas, pero no
deja verlas ni administrarlas):

- Al iniciar sesión en un sitio, la barra pregunta **¿Guardar la contraseña?**
  (o **¿Actualizarla?**, si cambió). *Nunca en este sitio* lo recuerda.
- Después, la **llave** de la barra de direcciones las rellena: con una sola
  cuenta, de un clic; con varias, eliges cuál. Nunca se rellenan solas al
  cargar la página, donde cualquier script del sitio las podría leer antes que
  tú.
- En **Ajustes › Contraseñas**: buscar, mostrar, copiar (se borra del
  portapapeles a los 30 segundos), editar, agregar a mano y borrar. Para ver,
  copiar o editar una, Windows pide tu PIN, huella o cara (**Windows Hello**),
  si lo tienes configurado.
- Se guardan en esta PC, **cifradas por Windows** con tu usuario (DPAPI): otra
  cuenta de la PC, o el archivo copiado a otra PC, no las puede leer. Solo se
  ofrece guardar en sitios con conexión segura (HTTPS), y una página solo puede
  ofrecer contraseñas de su propio sitio.
- Lo que no protege (igual que en Chrome o Edge): un programa que corre con tu
  mismo usuario de Windows sí puede descifrarlas, y Windows Hello frena a otra
  persona sentada frente a tu sesión abierta, no a un programa.

<p align="center">
  <img src="docs/img/contrasenas.jpg" alt="Ajustes, Contraseñas: la lista de sitios con su usuario y la contraseña oculta, con botones para mostrar, copiar, editar y borrar" width="100%">
</p>

### <img src="docs/icons/proceso.svg" width="22" height="22" alt=""> Si algo falla

- **Una página se cae** (un error del sitio, o se quedó sin memoria): en vez de
  quedar en blanco, la pestaña lo dice y ofrece **Volver a cargar**. La barra
  sigue mostrando la dirección del sitio, y si se cerró por falta de memoria,
  lo explica. Las demás pestañas ni se enteran.
- **Una página deja de responder** (un script que no termina): Michi pregunta
  si volver a cargarla o seguir esperando, como el *La página no responde* de
  Chrome.
- **Se cae el motor entero** (el proceso principal de WebView2): Michi guarda
  las pestañas y **se vuelve a abrir solo**, con todo como estaba. Si vuelve a
  pasar enseguida, avisa en vez de entrar en un bucle.
- **Se cae la barra o un panel**: se vuelve a cargar solo.
- **Al cerrar**, Michi guarda la sesión, cierra cada pestaña y espera a que
  WebView2 termine de escribir cookies y caché antes de salir (como mucho 3
  segundos), en vez de cortar el proceso de golpe.

<p align="center">
  <img src="docs/img/pagina-caida.gif" alt="Una pestaña de Wikipedia se cae, aparece la página Esta página dejó de funcionar y con Volver a cargar vuelve el sitio" width="100%">
</p>

### <img src="docs/icons/actualizar.svg" width="22" height="22" alt=""> Actualizaciones

Al iniciar (y como mucho dos veces por día) Michi le pregunta a GitHub cuál es
la última versión publicada. Si hay una más nueva, aparece **Actualizar** en la
barra, y en **Ajustes › Acerca de** el botón **Descargar e instalar**: baja el
instalador por HTTPS desde GitHub, comprueba que su tamaño y su huella SHA-256
sean los que publica GitHub para ese archivo y que venga **firmado con la clave
de Michi** (su `.sig`; la clave pública va dentro de Michi y la privada no está
en GitHub), lo abre y Michi se cierra para que pueda reemplazarlo. Si la firma
falta o no coincide, no se abre: así, ni quien se quedara con la cuenta de
GitHub podría colar un instalador. Nada se descarga ni se instala sin que lo
pidas, y la búsqueda automática se apaga en el mismo lugar (*Buscar ahora*
sigue andando).

<p align="center">
  <img src="docs/img/actualizaciones.jpg" alt="Ajustes, Acerca de: Michi 0.1.7, Actualizaciones: Michi está al día, y el interruptor para buscar al iniciar" width="100%">
</p>

### <img src="docs/icons/atajos.svg" width="22" height="22" alt=""> Atajos de teclado

| Acción | Atajo |
| --- | --- |
| Nueva pestaña | <kbd>Ctrl</kbd> + <kbd>T</kbd> |
| Cerrar pestaña | <kbd>Ctrl</kbd> + <kbd>W</kbd> (o clic con la rueda en el panel) |
| Reabrir pestaña cerrada | <kbd>Ctrl</kbd> + <kbd>Mayús</kbd> + <kbd>T</kbd> |
| Pestaña de incógnito | <kbd>Ctrl</kbd> + <kbd>Mayús</kbd> + <kbd>N</kbd> |
| Ir a la pestaña 1 a 8 / a la última | <kbd>Ctrl</kbd> + <kbd>1</kbd> … <kbd>8</kbd> / <kbd>Ctrl</kbd> + <kbd>9</kbd> |
| Pestaña siguiente / anterior | <kbd>Ctrl</kbd> + <kbd>Tab</kbd> / <kbd>Ctrl</kbd> + <kbd>Mayús</kbd> + <kbd>Tab</kbd> |
| Ir a la barra de direcciones | <kbd>Ctrl</kbd> + <kbd>L</kbd> o <kbd>Alt</kbd> + <kbd>D</kbd> |
| Recorrer las sugerencias de la barra | <kbd>↑</kbd> / <kbd>↓</kbd>, <kbd>Enter</kbd> para abrir, <kbd>Esc</kbd> para cerrar |
| Agregar / quitar marcador | <kbd>Ctrl</kbd> + <kbd>D</kbd> |
| Historial, descargas y marcadores | <kbd>Ctrl</kbd> + <kbd>H</kbd> |
| Menú | <kbd>Alt</kbd> + <kbd>F</kbd> |
| Buscar en la página | <kbd>Ctrl</kbd> + <kbd>F</kbd> |
| Recargar | <kbd>F5</kbd> |
| Zoom (se recuerda para cada sitio) | <kbd>Ctrl</kbd> + <kbd>+</kbd> / <kbd>-</kbd> / <kbd>0</kbd>, o <kbd>Ctrl</kbd> + rueda |
| Imprimir | <kbd>Ctrl</kbd> + <kbd>P</kbd> |
| Herramientas de desarrollador | <kbd>F12</kbd> |

### <img src="docs/icons/ajustes.svg" width="22" height="22" alt=""> Y además

- **Ajustes** (`app://localhost/settings`, desde el menú **⋮**): buscador
  predeterminado (Google, Bing, DuckDuckGo, Brave, Ecosia) y sus sugerencias, al iniciar (pestaña
  nueva o restaurar la sesión), fondo, zoom predeterminado, carpeta de
  descargas, privacidad y seguridad (con incógnito, contraseñas y formularios),
  rendimiento, extensiones y actualizaciones.
- **Menú ⋮**: nueva pestaña, pestaña de incógnito, reabrir cerrada, zoom, marcador, imprimir,
  herramientas de desarrollador, extensiones, ajustes, acerca de, salir.
- **Michi predeterminado**: puede abrir los enlaces y archivos `.html` de
  otras aplicaciones (lo registra el instalador). Si Michi ya está abierto, lo
  pedido se abre en **una pestaña nueva de esa misma ventana**, que pasa al
  frente: nunca un segundo navegador.
- **Lo controla May**: May, la asistente de voz del mismo autor, usa Michi
  como su navegador. Por un pipe local (nunca desde la red, y solo de tu
  usuario de Windows) puede listar, abrir, cambiar y cerrar pestañas, leer la
  página, hacer clic, escribir y sacar capturas, con los comandos DevTools de
  cada pestaña y sin abrir un puerto de depuración. Viene **apagado**: la
  primera vez que se conecta, Michi pregunta si lo permites (después se cambia
  en Ajustes > Privacidad y seguridad). Nunca llega a las páginas internas
  (ajustes, contraseñas) ni a tus pestañas de incógnito, solo a las que abre
  May, y mientras lo usa la barra muestra **May**.
- **Zoom por sitio**: si agrandas un sitio al 125%, todas sus páginas abren al
  125%, en esa pestaña y en las próximas (como en Chrome). Al volver al
  predeterminado, el sitio se olvida; en **Ajustes › Apariencia** se olvidan
  todos, y también al borrar el historial. Lo de incógnito no se recuerda.
- **Completar formularios**: sugiere lo que ya escribiste en campos parecidos
  (nombre, dirección, teléfono). Se apaga en **Ajustes › Privacidad**.
- **Sonido con nombre**: en el mezclador de volumen de Windows, el sonido de las
  páginas aparece como **Michi**, con su icono. Lo reproduce un proceso de
  WebView2 (`msedgewebview2.exe`), y Michi le pone su nombre a esa sesión de
  audio cuando una pestaña empieza a sonar.
- Ajustes, marcadores, historial, sesión y extensiones se guardan en
  `%APPDATA%\Michi`.

<br>

<a id="seguridad"></a>

## <img src="docs/icons/seguridad.svg" width="26" height="26" alt=""> Seguridad

Lo que ya trae el motor Chromium se deja tal cual: procesos en *sandbox*,
aislamiento de sitios, política de mismo origen, HSTS, bloqueo de contenido
mixto, validación de certificados y actualizaciones de seguridad automáticas
(WebView2 *Evergreen* se actualiza solo, con Windows Update). Encima de eso,
Michi agrega su propia capa (ver [`src/security.rs`](src/security.rs)).

### La barra de direcciones te dice con quién hablas

<p align="center">
  <img src="docs/img/barra-de-direcciones.png" alt="Cuatro estados de la barra: conexión segura, No seguro por HTTP, No seguro por certificado inválido, y el escudo de lo bloqueado" width="90%">
</p>

1. <img src="docs/icons/seguro.svg" width="18" height="18" alt=""> **Conexión segura** (HTTPS).
2. <img src="docs/icons/no-seguro-amarillo.svg" width="18" height="18" alt=""> **No seguro**, en amarillo: el sitio se abrió sin cifrar (HTTP).
3. <img src="docs/icons/no-seguro-rojo.svg" width="18" height="18" alt=""> **No seguro**, en rojo: el certificado del sitio no es válido.
4. <img src="docs/icons/escudo.svg" width="18" height="18" alt=""> **El escudo**: la página intentó algo que se le bloqueó (una ventana
   emergente o abrir una aplicación externa). Tocándolo se abre igual la
   ventana emergente, si la querías.

El **dominio se ve resaltado** y el resto de la dirección atenuado, sin un
`usuario:clave@` delante que pueda disfrazarlo, y en su forma ASCII (`xn--…`) si
usa letras de otro alfabeto que imitan a las latinas. Además, al pasar el cursor
por un enlace, abajo se ve **a dónde lleva de verdad**.

### HTTPS primero

Toda dirección `http://` de un sitio público se intenta antes por `https://`.
Si el sitio no lo ofrece, según lo que elijas en Ajustes:

```mermaid
flowchart LR
    A["Abres http://sitio.com"] --> B{"¿Responde por HTTPS?"}
    B -- Sí --> C["Se abre https://sitio.com"]
    B -- "No (error, certificado inválido,<br/>10 s sin respuesta o vuelve a HTTP)" --> D{"Modo"}
    D -- "Usar HTTPS cuando exista" --> E["Se abre por HTTP,<br/>marcado como No seguro"]
    D -- "Avisar antes" --> F["Página de aviso:<br/>Volver / Continuar"]
```

<p align="center">
  <img src="docs/img/aviso-https.jpg" alt="La página de aviso: Este sitio no admite conexiones seguras" width="100%">
</p>

No se tocan `localhost`, las IP, los nombres de la red local ni los puertos no
estándar.

### Todas las medidas

| | Medida | Qué hace |
| :-: | --- | --- |
| <img src="docs/icons/seguridad.svg" width="20" height="20" alt=""> | **SmartScreen** | Filtro de Microsoft contra phishing, malware y descargas peligrosas. La librería base (wry) lo apaga por defecto; Michi lo enciende en todas sus vistas. |
| <img src="docs/icons/https.svg" width="20" height="20" alt=""> | **HTTPS primero** | Intenta siempre la conexión cifrada (ver arriba). |
| <img src="docs/icons/ventanas.svg" width="20" height="20" alt=""> | **Bloqueo de ventanas emergentes** | Una pestaña nueva solo si la pide un clic. Nunca hacia archivos locales, `data:`, `javascript:` ni páginas internas (tampoco con *Abrir enlace en una pestaña nueva* del clic derecho). Las que abre una página con un clic conservan su `window.opener`, así los inicios de sesión en ventana emergente ("Continuar con Google") funcionan. |
| <img src="docs/icons/pestanas.svg" width="20" height="20" alt=""> | **Aviso de pantalla completa** | Cuando un sitio ocupa toda la pantalla, Michi dice cuál es y cómo salir (<kbd>Esc</kbd>): un sitio no puede dibujar una barra falsa con otra dirección sin que se note. |
| <img src="docs/icons/apps-externas.svg" width="20" height="20" alt=""> | **Aplicaciones externas** | Siempre se bloquean los esquemas con historial de ataques (`ms-msdt:`, `search-ms:`, `ms-officecmd:`, `ms-appinstaller:`, `hcp:`…) y cualquier otro que una página intente abrir sin un clic. |
| <img src="docs/icons/permisos.svg" width="20" height="20" alt=""> | **Permisos** | Bloquear cámara, micrófono, ubicación, etc. para todos los sitios; nunca a sitios sin HTTPS; notificaciones solo después de un clic; lista de lo que tiene guardado cada sitio. |
| <img src="docs/icons/descargas.svg" width="20" height="20" alt=""> | **Descargas peligrosas** | Ejecutables, scripts y accesos directos esperan tu decisión; imágenes, PDF, documentos o `.zip` se descargan sin preguntar. Lo que frena SmartScreen queda "Bloqueado". |
| <img src="docs/icons/marca-web.svg" width="20" height="20" alt=""> | **Marca de la Web** | Todo lo descargado lleva `Zone.Identifier`: Windows lo revisa antes de ejecutarlo y Office lo abre en Vista protegida. |
| <img src="docs/icons/firma.svg" width="20" height="20" alt=""> | **Firma de extensiones** | Los `.crx` se verifican como en Chrome (CRX3, RSA/ECDSA), tienen que ser exactamente la extensión pedida y nunca una versión más vieja que la instalada. |
| <img src="docs/icons/actualizar.svg" width="20" height="20" alt=""> | **Actualizaciones firmadas** | Un instalador descargado solo se abre con la firma de la clave de Michi (Ed25519), además de la huella de GitHub. |
| <img src="docs/icons/zip.svg" width="20" height="20" alt=""> | **Anti *zip bomba*** | Las extensiones se descomprimen con topes (512 MB, 20 000 archivos) en una carpeta temporal. |
| <img src="docs/icons/paginas-internas.svg" width="20" height="20" alt=""> | **Páginas internas blindadas** | CSP con *nonce*, sin poder mostrarse dentro de un iframe ajeno, y `Cross-Origin-Resource-Policy`. |
| <img src="docs/icons/ipc.svg" width="20" height="20" alt=""> | **IPC verificado** | Los comandos solo se aceptan según la URL real de la página que los envía (la pone WebView2, no se puede falsear), y cada página solo tiene los que usa. |
| <img src="docs/icons/seguimiento.svg" width="20" height="20" alt=""> | **Prevención de seguimiento** | Básica, equilibrada o estricta. |
| <img src="docs/icons/busqueda.svg" width="20" height="20" alt=""> | **Sugerencias de búsqueda** | Solo se le envía al buscador lo que es una búsqueda, nunca una dirección, y sin cookies. Se pueden apagar. |
| <img src="docs/icons/borrar.svg" width="20" height="20" alt=""> | **Borrar datos al cerrar** | Historial, logos de los sitios, cookies, caché y descargas; la sesión no se guarda. |
| <img src="docs/icons/proceso.svg" width="20" height="20" alt=""> | **Proceso y ejecutable** | DLL solo desde ubicaciones seguras, Control Flow Guard, compatible con CET, DEP, ASLR y el runtime de C incluido. |

<p align="center">
  <img src="docs/img/ajustes-seguridad.jpg" alt="Ajustes, sección Privacidad y seguridad" width="49%">
  <img src="docs/img/ajustes-permisos.jpg" alt="Ajustes, permisos de los sitios" width="49%">
</p>

<details>
<summary><img src="docs/icons/detalles.svg" width="18" height="18" alt=""> <b>Detalles técnicos de cada medida</b></summary>

<br>

**Navegación**

- **SmartScreen.** Si no se le pasan argumentos, wry arranca WebView2 con
  `--disable-features=…,msSmartScreenProtection`. Todos los WebView usan ahora
  los mismos argumentos sin eso (`security::BROWSER_ARGS`). WebView2 obedece
  además a la opción de Windows *SmartScreen para Microsoft Edge* (Seguridad de
  Windows › Control de aplicaciones y navegador › Protección basada en la
  reputación): si está apagada, SmartScreen no actúa, y Ajustes lo avisa.
- **HTTPS primero** se hace en `NavigationStarting`: la navegación a `http://`
  se cancela y se repite por `https://`; si el intento falla
  (`NavigationCompleted`, un error de certificado, una redirección de vuelta a
  HTTP o 10 s sin respuesta), se vuelve a HTTP y el host queda permitido por la
  sesión. Atrás/adelante/recargar no se desvían.
- **Ventanas emergentes**: la pestaña nueva la abre la app (para el motor, la
  abrió el usuario), así que sin el filtro de destinos una página se saltearía
  los bloqueos de Chromium para `file:`, `data:` y páginas internas.
- `window.close()` desde una página cierra su pestaña.

**Interfaz y páginas propias**

- La barra, los paneles y el menú no pueden navegar fuera de su página (ni
  soltándoles encima un enlace o un archivo): una página ajena ahí tendría
  todos los comandos de `ipc::dispatch`.
- El IPC se acepta según la URL del documento que lo envía y no según en qué
  página "está" la pestaña: si no, un sitio que navegaba hacia los Ajustes
  podía mandar en ese instante comandos como si fuera los Ajustes.
- La barra decide si una URL es interna con la respuesta de Rust (origen
  exacto), no buscando `app.localhost` en el texto.
- Los iconos de extensiones llevan una clave al azar por sesión: sin ella, un
  sitio podría averiguar qué extensiones hay instaladas.

**Extensiones** ([`src/crx.rs`](src/crx.rs))

- CRX3: todas las firmas tienen que ser válidas y una de las claves tiene que
  ser la que da el id de la extensión. Es lo que protege la descarga:
  Complementos de Edge entrega los paquetes por HTTP, sin cifrar.
- Una versión de la tienda más vieja que la instalada no se instala: alguien
  en la red podría servir una anterior, auténtica, con fallas ya arregladas.
- Los `.zip` y las carpetas se marcan como "sin firma" en la confirmación, y un
  `.crx` del disco como "firmado por su autor": cualquiera puede firmar el suyo.
- Si la tienda descarga un `.crx` (su propio botón de agregar), la descarga se
  cancela y el paquete se vuelve a pedir a la tienda oficial por su id: nunca
  se instala lo que haya entregado la página.
- La tienda de Chrome es una aplicación de una sola página: las navegaciones
  internas (`history.pushState`) se siguen con el evento `SourceChanged` de
  WebView2, así la barra muestra la dirección real y ofrece "Agregar a Michi".

**Descargas** ([`src/downloads.rs`](src/downloads.rs))

- Solo los tipos que pueden ejecutar algo (`.exe`, `.msi`, `.bat`, `.ps1`,
  `.js`, `.lnk`, imágenes de disco…) quedan en suspenso hasta que el usuario
  decide; imágenes, PDF, documentos o `.zip` se descargan sin preguntar.
- Una página que intente guardar uno de esos tipos por su cuenta (File System
  Access) no puede.
- Todo lo descargado lleva la marca de la web (`Zone.Identifier`), así Windows
  avisa antes de abrirlo.

**Proceso y ejecutable** ([`.cargo/config.toml`](.cargo/config.toml))

- `SetDefaultDllDirectories` + política de carga de imágenes: nada de DLL
  desde la carpeta actual, el PATH, carpetas de red o archivos de integridad baja.
- `/DEPENDENTLOADFLAG:0x800`: las DLL importadas, solo de System32.
- Control Flow Guard, `/CETCOMPAT`, `crt-static`; en release, `overflow-checks`.

</details>

<br>

<a id="privacidad"></a>

## <img src="docs/icons/usuario.svg" width="26" height="26" alt=""> Privacidad: qué sale de tu PC

Michi no tiene servidores propios ni manda nada a su autor: ni estadísticas, ni
errores, ni lo que visitas. Tus marcadores, historial, contraseñas y ajustes
quedan en `%APPDATA%\Michi`. Lo único que sale de la PC, además de las páginas
que abres:

| Qué | A quién | Para qué | Se apaga en |
| --- | --- | --- | --- |
| Lo que escribes en la barra (nunca una dirección) | El buscador que elijas | Sugerencias de búsqueda | Ajustes › General |
| Las direcciones que visitas y lo que descargas | Microsoft (SmartScreen) | Avisar de phishing y malware | Ajustes › Privacidad |
| Una consulta a la API de GitHub (sin datos tuyos) | GitHub | Saber si hay una versión nueva | Ajustes › Acerca de |
| El pedido de la extensión que instalas | Chrome Web Store o Complementos de Edge | Instalarla | (solo cuando instalas una) |

El motor, Microsoft Edge WebView2, es parte de Windows y se actualiza con
Windows Update; sus datos de diagnóstico siguen lo que elijas en *Configuración
de Windows › Privacidad y seguridad › Diagnóstico y comentarios*.

<br>

<a id="instalador"></a>

## <img src="docs/icons/instalador.svg" width="26" height="26" alt=""> Instalador

Michi tiene su propio instalador, hecho con la misma base que el navegador
y con su misma estética: el **fondo animado detrás del vidrio** (que cambia en
vivo al elegir otro), el zorro y los colores de su degradado.

**Descarga:** `Michi-Setup-0.1.8.exe` en
[Releases](https://github.com/kilorito2/michi/releases/latest). No está firmado
digitalmente, así que Windows puede mostrar el aviso de SmartScreen la primera
vez: *Más información › Ejecutar de todas formas*.

<table>
  <tr>
    <td width="50%"><img src="docs/img/instalador-bienvenida.jpg" alt="Instalador: bienvenida"></td>
    <td width="50%"><img src="docs/img/instalador-personalizar.jpg" alt="Instalador: personalizar fondo y buscador"></td>
  </tr>
  <tr>
    <td align="center"><b>1.</b> Bienvenida</td>
    <td align="center"><b>2.</b> Personalizar: fondo, buscador, accesos directos y carpeta</td>
  </tr>
  <tr>
    <td width="50%"><img src="docs/img/instalador-progreso.jpg" alt="Instalador: progreso"></td>
    <td width="50%"><img src="docs/img/instalador-listo.jpg" alt="Instalador: listo"></td>
  </tr>
  <tr>
    <td align="center"><b>3.</b> Copiando y verificando cada archivo</td>
    <td align="center"><b>4.</b> Listo: abrirlo o hacerlo predeterminado</td>
  </tr>
</table>

- <img src="docs/icons/usuario.svg" width="18" height="18" alt=""> **Sin permisos de administrador**: se instala solo para tu usuario, en
  `%LOCALAPPDATA%\Programs\Michi`.
- <img src="docs/icons/actualizar.svg" width="18" height="18" alt=""> **Actualizar o reinstalar** sin perder ajustes, marcadores, historial,
  extensiones ni el perfil.
- <img src="docs/icons/verificado.svg" width="18" height="18" alt=""> **Cada archivo se verifica** (SHA-256) antes de reemplazar al anterior: un
  instalador cortado no deja una instalación a medias.
- <img src="docs/icons/predeterminado.svg" width="18" height="18" alt=""> **Michi predeterminado**: lo registra para `http`, `https` y `.html`
  (Windows deja elegirlo en *Aplicaciones predeterminadas*; ningún programa puede
  ponerse solo).
- <img src="docs/icons/aplicaciones.svg" width="18" height="18" alt=""> Aparece en **Aplicaciones instaladas** y en *Aplicaciones predeterminadas*
  como **Michi** (el `.exe` lleva su nombre en la información de versión), con
  desinstalador.
- <img src="docs/icons/actualizar.svg" width="18" height="18" alt=""> **Quita la versión anterior** de cuando se llamaba "Navegador": su registro,
  accesos directos y archivos del programa. El perfil viejo (sesiones) se muda a
  Michi si todavía no tiene uno; si ya tiene, queda donde estaba: nunca se
  borran datos del usuario.
- <img src="docs/icons/seguro-naranja.svg" width="18" height="18" alt=""> Si el navegador está abierto, pide cerrarlo antes; si falta WebView2,
  ofrece descargarlo.

<details>
<summary><img src="docs/icons/desinstalador.svg" width="18" height="18" alt=""> <b>El desinstalador</b></summary>

<br>

<p align="center">
  <img src="docs/img/desinstalador.jpg" alt="Desinstalador de Michi" width="70%">
</p>

Es el mismo programa sin los archivos del navegador. Como un programa no puede
borrar su propio `.exe` mientras corre, se copia a la carpeta temporal y sigue
desde ahí. Borra **solo lo que instaló** (nunca una carpeta entera a ciegas) y,
si lo pides, también tus datos.

</details>

### Generar el instalador

```powershell
powershell -ExecutionPolicy Bypass -File tools\build_installer.ps1
```

Genera **`dist\Michi-Setup-<versión>.exe`** (~39 MB, un solo archivo):
compila en *release* el navegador y el instalador (`installer/`) y le pega al
final del instalador el `.exe` y los fondos, con el SHA-256 de cada uno.
Después lo firma para las actualizaciones (`dist\Michi-Setup-<versión>.exe.sig`,
con [`tools/firmar`](tools/firmar)): en la versión de GitHub se publican **los
dos archivos**, o Michi no la instala desde *Actualizar*.

La clave privada está en `%USERPROFILE%\.michi\clave-actualizaciones.pk8` (o
donde diga `MICHI_CLAVE_FIRMA`), nunca en el repositorio. Guarda una copia: si
se pierde, las versiones nuevas no se pueden firmar y hay que publicar una con
otra clave pública que los usuarios instalen a mano.

```text
┌────────────────┬──────────────┬─────┬──────────────┬───────────────┬──────────┬──────────┐
│  instalador    │  archivo 1   │ ... │  archivo N   │ índice (JSON) │ largo    │ MICHISET │
│  (.exe)        │  michi.exe   │     │ wallpapers/… │ rutas + SHA256│ (u64 LE) │ (firma)  │
└────────────────┴──────────────┴─────┴──────────────┴───────────────┴──────────┴──────────┘
```

> [!NOTE]
> El instalador no está firmado digitalmente, así que Windows puede advertir de
> un "editor desconocido". La firma gratuita para proyectos de código abierto
> (SignPath Foundation) no firma paquetes con contenido que no sea de código
> abierto, y el instalador lleva los fondos animados, que son de terceros.

<br>

<a id="compilar"></a>

## <img src="docs/icons/compilar.svg" width="26" height="26" alt=""> Compilar desde el código

### Requisitos

- **Windows 10 u 11** con [Microsoft Edge WebView2 Runtime](https://developer.microsoft.com/microsoft-edge/webview2/)
  (viene con Windows 11 y con casi todos los Windows 10).
- **Rust** ([rustup](https://rustup.rs/)) con el *toolchain* MSVC.
- **Visual Studio Build Tools** (C++ y el Windows SDK).
- Para regenerar los fondos: **ffmpeg** y **bash** (Git Bash).

### Pasos

```powershell
git clone <url-del-repo>
cd Michi

# Ejecutar en modo desarrollo
cargo run

# Pruebas
cargo test --workspace
```

> [!IMPORTANT]
> Los videos de fondo **no están en el repositorio** (`assets/wallpapers/` está
> en `.gitignore`). Para compilar hacen falta las versiones optimizadas en
> `assets/wallpapers/optimized/`: pon los originales en `assets/wallpapers/` y
> ejecuta `bash tools/encode_wallpapers.sh <archivo>` (ver
> [Fondos animados](#fondos-animados)). Sin ellos, el navegador compila igual
> (los fondos simplemente no se ven), pero el instalador no.

### Útil para desarrollar

| | |
| --- | --- |
| `MICHI_DATA_DIR=<carpeta>` | Usa otra carpeta de datos: para probar sin tocar tus datos reales. |
| `cargo test -- --ignored` | Además descarga extensiones reales de las dos tiendas y verifica su firma. |
| Consola | En *debug* el navegador muestra sus mensajes (`[seguridad]`, `[ipc]`); en *release* no abre consola. |
| `crash.log` | Si el navegador se cierra por un error, el detalle queda en la carpeta de datos. |
| `tools\make_icon.ps1` | Regenera el `.ico` y el logo desde `assets/icon/michi.png`. |

<br>

<a id="arquitectura"></a>

## <img src="docs/icons/arquitectura.svg" width="26" height="26" alt=""> Arquitectura

Cada pestaña es un **WebView real e independiente** (motor Chromium vía
WebView2), no un `<iframe>`. Esto es clave: un iframe no puede mostrar Google,
GitHub, bancos, etc., porque esos sitios bloquean ser incrustados. Con un
WebView nativo por pestaña, cualquier sitio funciona igual que en un navegador
normal.

La barra superior, los paneles y el menú son **otros WebViews**, con su propio
HTML/CSS/JS (carpeta [`ui/`](ui)). Rust es el centro: recibe los comandos de las
páginas (IPC) y les empuja el estado.

```mermaid
flowchart LR
    UI["<b>Interfaz</b><br/>barra · paneles · menú<br/>ajustes · pestaña nueva<br/><i>HTML propio en ui/</i>"]
    R["<b>Rust</b><br/>estado · comandos<br/>seguridad · descargas<br/>extensiones"]
    T["<b>Pestañas</b><br/>un WebView2 (Chromium)<br/>por pestaña<br/><i>cualquier sitio</i>"]
    UI -- "comandos (IPC)" --> R
    R -- "estado (evaluate_script)" --> UI
    T -- "eventos: navegación, permisos,<br/>descargas, ventanas nuevas" --> R
    R -- "decisiones: HTTPS primero,<br/>bloquear, permitir" --> T
```

<details>
<summary><img src="docs/icons/estructura.svg" width="18" height="18" alt=""> <b>Estructura del proyecto</b></summary>

```text
src/
  main.rs         punto de entrada, ventana, bucle de eventos (winit)
  state.rs        estado central (AppState) compartido vía Rc<RefCell<_>>
  storage.rs      ajustes/marcadores/historial/sesión en disco + buscadores
  chrome.rs       construye los WebViews de la interfaz
  menu.rs         menú ⋮ (un WebView propio que se muestra/oculta)
  tabs.rs         crear/cerrar/mover/fijar/silenciar pestañas (WebViews de contenido)
  groups.rs       grupos de pestañas
  layout.rs       posición y tamaño de cada WebView
  panels.rs       apertura/cierre animado de los paneles + vigilante del cursor
  actions.rs      acciones diferidas (atajos, menú) fuera de los callbacks
  control.rs      pipe de control para May: pestañas, navegación y comandos CDP
  pipes.rs        pipes con nombre solo para tu usuario de Windows
  shortcuts.rs    atajos de teclado (AcceleratorKeyPressed de WebView2)
  extensions.rs   instalar/listar/activar/quitar extensiones
  crx.rs          verificar la firma de los paquetes .crx (CRX3)
  popup.rs        popup de una extensión (su icono en la barra)
  downloads.rs    descargas con progreso / pausa / archivos peligrosos
  import.rs       importar marcadores/historial/cookies/pestañas de otros navegadores
  suggest.rs      sugerencias de la barra: completar en línea, buscador, historial
  library.rs      panel izquierdo: carpetas de marcadores, buscar en el historial
  context_menu.rs menú del clic derecho de las páginas
  notice.rs       aviso de pantalla completa
  zoom.rs         zoom por sitio
  recovery.rs     página caída, página colgada, reabrir si se cae WebView2
  passwords.rs    gestor de contraseñas (DPAPI, Windows Hello, rellenar)
  update.rs       buscar, descargar y verificar versiones nuevas en GitHub
  audio.rs        nombre e icono de Michi en el mezclador de volumen
  security.rs     medidas de seguridad
  permissions.rs  permisos de los sitios
  perf.rs         modos de rendimiento: pestañas dormidas, memoria, primer plano
  win.rs          maximizar sin tapar la barra de tareas
  single_instance.rs  un solo Michi: los enlaces/archivos van al que ya está abierto
  native.rs       API de WebView2 que wry no expone
  protocol.rs     protocolo app:// (páginas propias y fondos)
  ipc.rs          comandos que la interfaz (JS) envía a Rust
  sync.rs         Rust → interfaz: empuja el estado
ui/
  topbar.html         barra superior
  sidebar_right.html  panel de pestañas
  sidebar_left.html   panel de descargas/historial/marcadores
  menu.html           menú ⋮
  suggest.html        desplegable de sugerencias de la barra
  settings.html       ajustes y extensiones
  newtab.html         pestaña nueva
  insecure.html       aviso "este sitio no admite conexiones seguras"
  crashed.html        "esta página dejó de funcionar"
  notice.html         aviso "<sitio> está en pantalla completa"
  ext_popup.js        se inyecta en el popup de una extensión
  content_init.js     se inyecta en cada pestaña
  wallpaper.js        fondo animado compartido
installer/            el instalador
  src/main.rs         ventana, interfaz, instalar/desinstalar en otro hilo
  src/install.rs      los pasos de instalar y desinstalar
  src/payload.rs      los archivos pegados al .exe (y su verificación)
  src/system.rs       registro, accesos directos, procesos
  ui/index.html       la interfaz del instalador
.github/workflows/
  compilar.yml        compila y corre las pruebas en GitHub con cada cambio
tools/
  encode_wallpapers.sh  genera assets/wallpapers/optimized/
  make_icon.ps1         genera michi.ico y michi-256.png
  build_installer.ps1   genera dist\Michi-Setup-<versión>.exe (y su .sig)
  firmar/               firma los instaladores para las actualizaciones
assets/icon/          el zorro: original, .ico y logo
docs/img/             imágenes de este README
```

</details>

**Motor:** [wry 0.57](https://github.com/tauri-apps/wry) (WebView2) +
[winit 0.30](https://github.com/rust-windowing/winit) (ventana y eventos).

<br>

<a id="decisiones"></a>

## <img src="docs/icons/decisiones.svg" width="26" height="26" alt=""> Decisiones de diseño

<details>
<summary><img src="docs/icons/fondos.svg" width="18" height="18" alt=""> <b id="fondos-animados">Fondos animados: cómo se ven como una sola imagen</b></summary>

<br>

Cada superficie (barra, paneles, menú, pestaña nueva) es un WebView distinto y
decodifica su propio `<video>`, así que el costo se multiplica. Por eso la app no
sirve los originales 4K de `assets/wallpapers/`, sino las versiones de
`assets/wallpapers/optimized/` que genera `tools/encode_wallpapers.sh`:

- `<tema>.mp4`: 1920×1080 a 60 fps (los originales a 30 fps se interpolan), para
  la pestaña nueva.
- `<tema>.blur.mp4`: 640×360 a 60 fps con el desenfoque y la saturación del
  vidrio ya aplicados, para la barra y los paneles (antes lo hacía
  `backdrop-filter` en cada cuadro).

`protocol.rs` lee solo el rango pedido por el `<video>`, en un hilo aparte.
`ui/wallpaper.js` sincroniza todas las superficies con el reloj del sistema (para
que se vean como un único fondo), cambia de tema sin dejar un hueco mientras
carga el nuevo pausa el video de las pestañas en segundo plano y congela los fondos mientras la
pestaña al frente reproduce un video o sonido (`ui/content_init.js` avisa cuando
hay un `<video>` reproduciéndose; `sync::push_media` lo reparte). Un fondo
también puede ser una imagen fija (`.jpg`), animada con un zoom lento por CSS.

Para agregar un fondo: poner el original en `assets/wallpapers/`, sumarlo a
`JOBS` en el script y a `THEMES` en `protocol.rs` (y a `STILLS` si es una
imagen) y correr `bash tools/encode_wallpapers.sh <archivo original>`.

</details>

<details>
<summary><img src="docs/icons/paneles.svg" width="18" height="18" alt=""> <b>Paneles que se deslizan sin parpadear</b></summary>

<br>

El WebView de cada panel mide siempre 300 px; cerrado queda casi entero fuera de
la ventana y solo asoma una franja de 10 px en el borde. La página del panel
avisa cuando el cursor **entra**, y Rust lo desliza hacia adentro **moviendo** el
WebView cuadro a cuadro: nunca lo redimensiona, porque WebView2 no repinta
mientras se lo redimensiona, pero moverlo es gratis. El **cierre** lo decide Rust
(`src/panels.rs`) mirando la posición real del cursor, y lo cierra cuando lleva
~200 ms fuera (`mouseleave` se perdía al mover el mouse rápido).

Como wry no expone el orden de apilamiento entre WebViews, `layout::relayout`
vuelve a subir la barra y los paneles al tope con `SetWindowPos` cada vez que
cambia algo.

</details>

<details>
<summary><img src="docs/icons/diferido.svg" width="18" height="18" alt=""> <b>Por qué casi todo es "diferido"</b></summary>

<br>

Las pestañas nuevas se crean en la próxima vuelta del bucle de eventos, no
dentro del callback de WebView2 que las pide (botón **+**, `target=_blank`):
crear un WebView ahí adentro se colgaba (ver `AppState::request_tab`). Lo mismo
con cerrar pestañas o navegar desde un evento: se encolan como `Action` y corren
fuera de los callbacks.

Las páginas de interfaz y la pestaña nueva se sirven por el protocolo `app://`
(no con `with_html`), para que la ruta relativa del video de fondo resuelva y se
vea el mismo fondo en toda la interfaz.

</details>

<details>
<summary><img src="docs/icons/oscuro.svg" width="18" height="18" alt=""> <b>La portada de Google, en oscuro</b></summary>

<br>

`ui/content_init.js` se inyecta en cada pestaña, pero solo actúa cuando la página
es exactamente la portada de Google: la deja en un tono oscuro liso que combina
con la interfaz, con solo la barra de búsqueda. Se probó un fondo realmente
transparente, pero WebView2 reservaba ahí una región blanca opaca que no
respondía a CSS, así que se optó por un color sólido.

</details>

<br>

<a id="limitaciones"></a>

## <img src="docs/icons/limitaciones.svg" width="26" height="26" alt=""> Limitaciones conocidas

- **Las contraseñas que guardó la versión 0.1.7** (en el gestor de WebView2) no
  se pueden pasar al gestor nuevo: WebView2 no deja leerlas. Hay que volver a
  guardarlas (Michi pregunta la próxima vez que inicies sesión).
- **El instalador no está firmado**, así que Windows puede mostrar el aviso de
  "editor desconocido" la primera vez (las actualizaciones desde Michi lo abren
  directamente). Ver la nota de la sección [Instalador](#instalador).
- **Extensiones sin popup** (las que hacen algo directo al tocar su icono) no
  aparecen en la barra: WebView2 no permite disparar ese evento desde afuera.
- **SmartScreen depende de Windows**: si *SmartScreen para Microsoft Edge* está
  apagado en Seguridad de Windows, WebView2 no lo usa.
- **Mezcladores de audio de terceros** (SteelSeries Sonar y similares) pueden
  seguir mostrando `msedgewebview2` si identifican el sonido por el nombre del
  proceso en vez del nombre de la sesión de audio. El mezclador de Windows
  muestra "Michi".
- **Incógnito es una pestaña, no una ventana aparte**: comparte la ventana con las normales
  (se distingue por el color y el fantasma), y <kbd>Ctrl</kbd> + <kbd>T</kbd> o el botón **+** abren siempre una pestaña
  normal, incluso desde una de incógnito. Las extensiones pueden no funcionar en ellas:
  WebView2 las trata aparte en el perfil InPrivate.
- **El modo Optimizado pausa el fondo con Michi fuera de primer plano**, aunque
  la ventana siga visible al lado de otra aplicación.
- El efecto vidrio difumina el fondo animado, no los píxeles de la página de
  atrás: WebView2 no permite mezclar el contenido de dos WebViews.

<br>

<a id="hoja-de-ruta"></a>

## <img src="docs/icons/hoja-de-ruta.svg" width="26" height="26" alt=""> Hoja de ruta

- [x] Carpetas y edición de marcadores; buscar en el historial.
- [x] Modo incógnito (pestañas con perfil InPrivate de WebView2).
- [x] Modos de rendimiento: optimizado y super optimizado.
- [x] Reordenar pestañas arrastrándolas en el panel; fijarlas y silenciarlas.
- [x] Una sola instancia: los enlaces de otras aplicaciones abren una pestaña en la ventana abierta.
- [x] Sugerencias en la barra de direcciones.
- [x] Menú del clic derecho con pestaña nueva, incógnito y buscar la selección.
- [x] Recuperarse solo de una página o un motor caídos; cierre ordenado.
- [x] Actualizaciones desde GitHub, verificadas con SHA-256.
- [x] Zoom por sitio; guardar contraseñas y completar formularios.
- [x] Grupos de pestañas.
- [x] Gestor de contraseñas: ver, buscar, copiar, editar y borrar, con Windows Hello.
- [x] Compilar y probar en GitHub Actions con cada cambio.
- [ ] Firmar el instalador (gratis con SignPath Foundation, si el instalador deja
      de llevar los fondos de terceros).

<br>

## Licencia

El código de Michi se publica bajo la [licencia MIT](LICENSE).

Los fondos animados que trae el instalador son obra de sus respectivos autores
y no forman parte del código del repositorio ni de esta licencia.

<br>

<div align="center">

<img src="assets/icon/michi-256.png" alt="" width="72">

**Michi** · hecho en Rust con WebView2

</div>
