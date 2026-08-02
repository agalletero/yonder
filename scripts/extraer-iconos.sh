#!/bin/bash
# Extrae el subconjunto de iconos Lucide que usa la aplicación.
#
# Los SVG de Lucide llevan stroke="currentColor". resvg (el rasterizador que usa
# egui) no resuelve currentColor, así que se sustituye por blanco puro: un icono
# blanco se tinta a cualquier color multiplicando, que es como egui aplica
# Image::tint(). El color real sale siempre de los tokens del tema.
#
# Fuente: https://github.com/lucide-icons/lucide (licencia ISC)
#
# Uso:
#   scripts/extraer-iconos.sh                 # descarga Lucide de internet
#   scripts/extraer-iconos.sh <DIR_LUCIDE>    # usa una copia local ya descargada

set -euo pipefail

BASE="$(cd "$(dirname "$0")/.." && pwd)"
DESTINO="$BASE/assets/iconos"
REPO_URL="https://github.com/lucide-icons/lucide/archive/refs/heads/main.tar.gz"

# Iconos usados por la aplicación. Uno por línea, sin extensión.
ICONOS=(
    # Identidad y navegación
    waypoints server network cable globe terminal panel-left list
    # Acciones
    play square plus pencil trash-2 copy save search x check
    refresh-cw power download settings sliders-horizontal
    chevron-right chevron-down chevron-up ellipsis
    # Estados de la máquina de §4
    circle-dot loader-circle circle-check triangle-alert circle-x circle-alert
    # Seguridad
    shield-check shield-alert key-round lock lock-open usb file-key
    # Información
    info clock history activity gauge chart-line zap link-2 arrow-right-left
    # Tema
    sun moon monitor-cog
)

TMP_DIR=""
limpiar() { if [ -n "$TMP_DIR" ]; then rm -rf "$TMP_DIR"; fi; return 0; }
trap limpiar EXIT

if [ $# -ge 1 ]; then
    ORIGEN="$1"
    echo "[INFO] Usando copia local de Lucide: $ORIGEN"
else
    TMP_DIR="$(mktemp -d)"
    echo "[INFO] Descargando Lucide desde $REPO_URL"
    curl -sL "$REPO_URL" -o "$TMP_DIR/lucide.tar.gz"
    tar xzf "$TMP_DIR/lucide.tar.gz" -C "$TMP_DIR"
    ORIGEN="$TMP_DIR/lucide-main/icons"
fi

if [ ! -d "$ORIGEN" ]; then
    echo "[ERR] No existe el directorio de origen: $ORIGEN" >&2
    exit 1
fi

mkdir -p "$DESTINO"
FALTAN=0
COPIADOS=0

for icono in "${ICONOS[@]}"; do
    ORIG="$ORIGEN/$icono.svg"
    if [ ! -f "$ORIG" ]; then
        echo "[WARN] No encontrado en Lucide: $icono"
        FALTAN=$((FALTAN + 1))
        continue
    fi
    sed 's/stroke="currentColor"/stroke="#ffffff"/g' "$ORIG" > "$DESTINO/$icono.svg"
    COPIADOS=$((COPIADOS + 1))
done

echo "[INFO] $COPIADOS iconos escritos en assets/iconos/"
if [ "$FALTAN" -gt 0 ]; then
    echo "[WARN] $FALTAN iconos no se encontraron; revisa la lista ICONOS"
    exit 1
fi

exit 0
