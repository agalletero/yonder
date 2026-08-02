#!/bin/bash
# Construye el paquete .deb con dpkg-deb, sin más herramientas.
#
# Uso:
#   cargo build --release
#   empaquetado/construir-deb.sh            # deja el .deb en dist/
#   empaquetado/construir-deb.sh <destino>
#
# Sobre las dependencias: `ldd` sobre el binario solo enseña libc, libm y
# libgcc_s, porque winit y glow cargan las bibliotecas gráficas con dlopen en
# tiempo de ejecución. Si el paquete se fiara de lo que dice `ldd`, se instalaría
# limpiamente en una máquina donde la ventana no puede abrirse. De ahí que las
# gráficas estén declaradas a mano; la lista sale de:
#
#   strings -a target/release/yonder | grep -oE 'lib[A-Za-z0-9_+-]+\.so(\.[0-9]+)*'

set -euo pipefail

RAIZ="$(cd "$(dirname "$0")/.." && pwd)"
# shellcheck source=empaquetado/comun.sh
source "$RAIZ/empaquetado/comun.sh"

DESTINO="${1:-$RAIZ/dist}"
VERSION="$(version_del_proyecto)"
ARQUITECTURA="$(dpkg --print-architecture 2>/dev/null || echo amd64)"
PAQUETE="yonder_${VERSION}_${ARQUITECTURA}.deb"

exigir_binarios

if ! command -v dpkg-deb >/dev/null 2>&1; then
    echo "[ERR] falta dpkg-deb (paquete dpkg)" >&2
    exit 1
fi

TRABAJO="$(mktemp -d)"
trap 'rm -rf "$TRABAJO"' EXIT

poblar_arbol "$TRABAJO"
mkdir -p "$TRABAJO/DEBIAN"

# Tamaño instalado en KiB, como manda la política de Debian.
TAMANO="$(du -sk "$TRABAJO" | cut -f1)"

{
    cat <<CONTROL
Package: yonder
Version: $VERSION
Section: net
Priority: optional
Architecture: $ARQUITECTURA
Maintainer: Alex Galletero <alex.galletero.quer@gmail.com>
Installed-Size: $TAMANO
Depends: libc6 (>= 2.35), libgcc-s1, openssh-client, libgl1, libx11-6,
 libxkbcommon0, libxkbcommon-x11-0, libwayland-client0, libwayland-egl1
Recommends: gnome-keyring | kwalletmanager
Description: $(descripcion_del_proyecto)
CONTROL

    # La descripción larga de Debian lleva un espacio delante de cada línea y un
    # punto solo en las líneas vacías.
    cuerpo_descripcion | sed -e 's/^$/./' -e 's/^/ /'
} > "$TRABAJO/DEBIAN/control"

# Sin postinst que toque nada del sistema: actualizar las cachés de iconos y de
# escritorio ya lo hacen los disparadores de dpkg en Debian y derivadas.

mkdir -p "$DESTINO"
dpkg-deb --root-owner-group --build "$TRABAJO" "$DESTINO/$PAQUETE" >/dev/null

echo "[INFO] $DESTINO/$PAQUETE"
dpkg-deb --info "$DESTINO/$PAQUETE" | sed 's/^/  /'
echo "[INFO] contenido:"
dpkg-deb --contents "$DESTINO/$PAQUETE" | awk '{print "  " $6}'
