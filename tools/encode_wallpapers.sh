#!/usr/bin/env bash
# Genera las versiones que la app sirve de verdad (ver src/protocol.rs) a
# partir de los originales de assets/wallpapers/*.mp4:
#
#   optimized/<nombre>.mp4       1920x1080 a 60fps: fondo de la pestana nueva.
#   optimized/<nombre>.blur.mp4  640x360 a 60fps, YA difuminado y saturado:
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
# GOP fijo de 1s (-g 60, sin cortes por escena): el `loop` del <video> salta
# al inicio y el sync entre superficies (ui/wallpaper.js) hace un seek; con
# keyframes frecuentes ambos son casi instantaneos.
#
# Uso: bash tools/encode_wallpapers.sh [nombre_original.mp4 ...]
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
      -g 60 -keyint_min 60 -sc_threshold 0 -movflags +faststart -an)

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
    "${X264[@]}" -crf 20 -level 4.2 -maxrate 12M -bufsize 24M "$full.tmp.mp4"
  mv "$full.tmp.mp4" "$full"

  # sigma 10 a 640px ~= blur(22px) de CSS en una ventana de ~1366px de ancho.
  ffmpeg -hide_banner -loglevel error -y -i "$full" \
    -vf "scale=640:360:flags=area,gblur=sigma=10,eq=saturation=1.5" \
    "${X264[@]}" -crf 24 -level 4.0 "$blur.tmp.mp4"
  mv "$blur.tmp.mp4" "$blur"
  echo "listo: $name"
}

pids=()
for job in "${JOBS[@]}"; do
  name="${job%%:*}" src="${job#*:}"
  if [ $# -gt 0 ] && [[ ! " $* " == *" $src "* ]]; then continue; fi
  encode "$name" "$src" &
  pids+=($!)
done
status=0
for p in "${pids[@]}"; do wait "$p" || status=1; done
exit $status
