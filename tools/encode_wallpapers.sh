#!/usr/bin/env bash
# Genera las versiones que la app sirve de verdad (ver src/protocol.rs) a
# partir de los originales de assets/wallpapers/*.mp4:
#
#   optimized/<nombre>.mp4       1920x1080 a 60fps: fondo de la pestana nueva.
#   optimized/<nombre>.blur.mp4  320x180 a 30fps, YA difuminado y saturado:
#                                barra superior y paneles laterales.
#
# Si el original es una imagen (.jpg/.png) se generan <nombre>.jpg y
# <nombre>.blur.jpg en su lugar (ver encode_still).
#
# Por que: los originales son 4K/4096px (uno a 60fps y 36 Mbps), y cada
# superficie de la interfaz (barra, 2 paneles, pestana nueva) decodifica su
# propio <video> — 4 decodificaciones 4K simultaneas eran las que tiraban
# los cuadros. La barra y los paneles antes difuminaban el video con
# backdrop-filter en cada cuadro; horneando ese mismo blur+saturate en el
# video se ve igual y la GPU no tiene que recalcularlo 60 veces por segundo.
#
# Los originales a 30fps se interpolan a 60 (minterpolate, compensacion de
# movimiento). Es lento (~35x tiempo real a 1080p), por eso se corre una sola
# vez y los 4 videos en paralelo.
#
# El difuminado va a 30fps y 320x180: la barra y los dos paneles lo
# decodifican los tres a la vez todo el tiempo (un panel cerrado igual asoma
# por su franja), y con el desenfoque horneado no queda detalle que distinga
# 60 de 30fps ni 640 de 320px (comparado ampliado a 1440p): es la octava
# parte del trabajo por superficie.
#
# GOP fijo de 1s (-g 60 / -g 30, sin cortes por escena): el `loop` del
# <video> salta al inicio y el sync entre superficies (ui/wallpaper.js) hace
# un seek; con keyframes frecuentes ambos son casi instantaneos.
#
# Uso: bash tools/encode_wallpapers.sh [nombre_original.mp4 ...]
#      bash tools/encode_wallpapers.sh --blur-only
#        (rehace solo los .blur.mp4 a partir de los optimized/<nombre>.mp4 ya
#        generados; no necesita los originales)
set -euo pipefail
cd "$(dirname "$0")/../assets/wallpapers"
mkdir -p optimized

# nombre expuesto (el de protocol.rs) : archivo original
JOBS=(
  "sleepy-rainy-evening:sleepy-rainy-evening.3840x2160.mp4"
  "torii-carmesi:torii-carmesi.jpg"
  "blindfolded-girl:blindfolded-girl-blue-moonlight-glowing-petals-live-wallpaper.mp4"
  "wuthering-waves-chisa:wuthering-waves-chisa-live-wallpaper.mp4"
)

X264=(-c:v libx264 -preset slow -profile:v high -pix_fmt yuv420p
      -sc_threshold 0 -movflags +faststart -an)
GOP60=(-g 60 -keyint_min 60)
GOP30=(-g 30 -keyint_min 30)

# Imagen fija (la anima ui/wallpaper.js con un zoom por CSS). 2560px de ancho
# y no 1920: ese zoom la agranda hasta un 8%.
encode_still() {
  local name="$1" src="$2"
  ffmpeg -hide_banner -loglevel error -y -i "$src" \
    -vf "scale=2560:1440:flags=lanczos" -q:v 2 "optimized/$name.tmp.jpg"
  mv "optimized/$name.tmp.jpg" "optimized/$name.jpg"
  ffmpeg -hide_banner -loglevel error -y -i "$src" \
    -vf "scale=640:360:flags=area,gblur=sigma=10,eq=saturation=1.5" -q:v 3 "optimized/$name.blur.tmp.jpg"
  mv "optimized/$name.blur.tmp.jpg" "optimized/$name.blur.jpg"
  echo "listo: $name"
}

encode() {
  local name="$1" src="$2"
  case "$src" in *.jpg|*.jpeg|*.png) encode_still "$name" "$src"; return ;; esac
  local full="optimized/$name.mp4" blur="optimized/$name.blur.mp4"
  local fps
  fps=$(ffprobe -v error -select_streams v:0 -show_entries stream=r_frame_rate -of csv=p=0 "$src")

  local vf="scale=1920:1080:flags=lanczos"
  if [ "$fps" != "60/1" ]; then
    vf="$vf,minterpolate=fps=60:mi_mode=mci:mc_mode=aobmc:me_mode=bidir:vsbmc=1"
  fi
  ffmpeg -hide_banner -loglevel error -y -i "$src" -vf "$vf" -r 60 \
    "${X264[@]}" "${GOP60[@]}" -crf 20 -level 4.2 -maxrate 12M -bufsize 24M "$full.tmp.mp4"
  mv "$full.tmp.mp4" "$full"
  encode_blur "$name"
}

# El difuminado, a partir del de 1080p. sigma 5 a 320px (= sigma 10 a 640px)
# ~= blur(22px) de CSS en una ventana de ~1366px de ancho.
#
# Tiene que durar EXACTAMENTE lo mismo que el de 1080p: cada superficie se
# ubica en (reloj % duracion) (ver ui/wallpaper.js), y con el reloj en miles
# de millones de segundos, una diferencia de 0.0003s ya los desfasa del
# todo. Por eso no `fps=30` (con una cantidad impar de cuadros, 30fps no
# puede durar lo mismo) sino uno de cada dos cuadros con su tiempo original
# (los del original: los que invento minterpolate quedan afuera), mas el
# ultimo, que dura lo que le falta. Sin cuadros B: corren un poco la
# duracion (la lista de edicion del mp4 se redondea).
encode_blur() {
  local name="$1"
  local full="optimized/$name.mp4" blur="optimized/$name.blur.mp4"
  local frames timescale
  frames=$(ffprobe -v error -select_streams v:0 -show_entries stream=nb_frames -of csv=p=0 "$full")
  timescale=$(ffprobe -v error -select_streams v:0 -show_entries stream=time_base -of csv=p=0 "$full")
  ffmpeg -hide_banner -loglevel error -y -i "$full" \
    -vf "select='not(mod(n\,2))+gte(n\,$((frames - 1)))',scale=320:180:flags=area,gblur=sigma=5,eq=saturation=1.5" \
    -fps_mode passthrough "${X264[@]}" "${GOP30[@]}" -bf 0 -crf 24 -level 4.0 \
    -video_track_timescale "${timescale#1/}" "$blur.tmp.mp4"
  mv "$blur.tmp.mp4" "$blur"
  local a b
  a=$(ffprobe -v error -show_entries format=duration -of csv=p=0 "$full")
  b=$(ffprobe -v error -show_entries format=duration -of csv=p=0 "$blur")
  if [ "$a" != "$b" ]; then
    echo "ERROR: $name dura $b y el de 1080p $a: se desincronizarian" >&2
    return 1
  fi
  echo "listo: $name"
}

blur_only=0
if [ "${1:-}" = "--blur-only" ]; then blur_only=1; shift; fi
pids=()
for job in "${JOBS[@]}"; do
  name="${job%%:*}" src="${job#*:}"
  if [ $blur_only = 1 ]; then
    case "$src" in *.jpg|*.jpeg|*.png) ;; *) encode_blur "$name" & pids+=($!) ;; esac
    continue
  fi
  if [ $# -gt 0 ] && [[ ! " $* " == *" $src "* ]]; then continue; fi
  encode "$name" "$src" &
  pids+=($!)
done
status=0
for p in "${pids[@]}"; do wait "$p" || status=1; done
exit $status
