#!/bin/bash
# Comprueba que el .deb es instalable y que lo que instala queda usable.
#
# Uso:
#   cargo build --release
#   empaquetado/construir-deb.sh
#   pruebas/paquete.sh dist/yonder_*_amd64.deb
#
# Se instala en una raíz temporal bajo fakeroot: no toca el sistema. Un paquete
# que se construye sin errores puede seguir siendo inservible —binario en el
# sitio equivocado, askpass que no acompaña al principal, permisos mal—, y eso
# solo se ve instalándolo.

set -u

if [ $# -lt 1 ]; then
    echo "[ERR] uso: $0 <fichero.deb>" >&2
    exit 1
fi

DEB="$(readlink -f "$1")"
FALLOS=0

comprobar() {
    if eval "$2"; then
        echo "  [OK]  $1"
    else
        echo "  [ERR] $1"
        FALLOS=$((FALLOS + 1))
    fi
}

for herramienta in fakeroot dpkg dpkg-deb; do
    if ! command -v "$herramienta" >/dev/null 2>&1; then
        echo "[WARN] falta «$herramienta»: se omite la prueba del paquete"
        exit 0
    fi
done

RAIZ="$(mktemp -d)"
trap 'rm -rf "$RAIZ"' EXIT
mkdir -p "$RAIZ"/var/lib/dpkg/{info,updates,triggers} "$RAIZ"/var/log
: > "$RAIZ/var/lib/dpkg/status"
: > "$RAIZ/var/lib/dpkg/available"

echo "########## instalación en una raíz temporal ##########"
fakeroot dpkg --root="$RAIZ" --force-depends --install "$DEB" 2>&1 \
    | grep -viE "depèn de|depends on|no està instal·lat|is not installed" \
    | sed 's/^/  /'

echo
echo "########## comprobaciones ##########"
comprobar "el binario principal queda en el PATH del sistema" \
    "[ -x '$RAIZ/usr/bin/yonder' ]"
# El principal busca al askpass en su mismo directorio; si el paquete los
# separara, los hosts con contraseña dejarían de funcionar desde la ventana.
comprobar "el askpass queda a su lado (§5.1)" \
    "[ -x '$RAIZ/usr/bin/yonder-askpass' ]"
comprobar "el binario instalado arranca" \
    "'$RAIZ/usr/bin/yonder' --version >/dev/null 2>&1"
comprobar "hay entrada en el menú de aplicaciones" \
    "[ -f '$RAIZ/usr/share/applications/yonder.desktop' ]"
comprobar "hay icono escalable" \
    "[ -f '$RAIZ/usr/share/icons/hicolor/scalable/apps/yonder.svg' ]"
comprobar "se instala la licencia" \
    "[ -f '$RAIZ/usr/share/doc/yonder/LICENSE' ]"

if command -v desktop-file-validate >/dev/null 2>&1; then
    comprobar "la entrada de menú pasa la validación de freedesktop" \
        "desktop-file-validate '$RAIZ/usr/share/applications/yonder.desktop'"
fi

echo
echo "########## dependencias declaradas ##########"
# Las gráficas se cargan con dlopen y no salen en `ldd`: si no estuvieran
# declaradas a mano, el paquete se instalaría limpiamente en una máquina donde
# la ventana no puede abrirse.
DEPENDENCIAS="$(dpkg-deb --field "$DEB" Depends)"
echo "$DEPENDENCIAS" | tr ',' '\n' | sed 's/^ *//' | sed 's/^/  /'
for necesaria in openssh-client libgl1 libx11-6 libxkbcommon0 libwayland-client0; do
    comprobar "se declara «$necesaria»" \
        "echo '$DEPENDENCIAS' | grep -q '$necesaria'"
done

echo
echo "########## desinstalación ##########"
fakeroot dpkg --root="$RAIZ" --force-depends --purge yonder >/dev/null 2>&1
QUEDAN="$(find "$RAIZ/usr" -type f 2>/dev/null | wc -l)"
comprobar "purgar no deja ningún fichero atrás" "[ '$QUEDAN' -eq 0 ]"

echo
if [ "$FALLOS" -eq 0 ]; then
    echo "=========== EL PAQUETE ESTÁ BIEN ==========="
else
    echo "=========== $FALLOS COMPROBACIONES FALLARON ==========="
    exit 1
fi
