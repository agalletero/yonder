#!/bin/bash
# Lanza la prueba de extremo a extremo dentro de un espacio de nombres con el
# directorio de inicio sustituido.
#
# Hace falta aislarlo de verdad, y no basta con exportar HOME, porque `ssh`
# expande el «~» de `~/.ssh/config` usando la entrada de passwd del usuario, no
# la variable de entorno. Sin un montaje real la prueba escribiría en la
# configuración SSH de verdad de quien la ejecute.
#
# Requiere `bwrap` (paquete bubblewrap) y espacios de nombres de usuario sin
# privilegios. Si no están disponibles, la prueba se salta con un aviso en vez
# de tocar nada.
#
# Uso:
#   pruebas/e2e-aislado.sh

set -eu

BASE="$(cd "$(dirname "$0")/.." && pwd)"
TRABAJO="${TMPDIR:-/tmp}/yonder-e2e-$$"
HOGAR_FALSO="$TRABAJO/hogar"
BINARIOS="$TRABAJO/bin"

if ! command -v bwrap >/dev/null 2>&1; then
    echo "[WARN] falta «bwrap» (paquete bubblewrap): se omite la prueba de extremo a extremo"
    echo "[WARN] sin aislamiento real escribiría en tu ~/.ssh, así que no se ejecuta"
    exit 0
fi

# No basta con que el binario exista: tiene que poder crear el espacio de
# nombres. Debian y Ubuntu 24.04 en adelante restringen los espacios de nombres
# de usuario sin privilegios mediante AppArmor, y ahí «bwrap» está instalado
# pero falla con «setting up uid map: Permission denied». Comprobarlo de verdad
# es lo que hace que esta prueba se salte en vez de morir, que es lo prometido
# tres líneas más arriba.
if ! bwrap --dev-bind / / --die-with-parent true >/dev/null 2>&1; then
    echo "[WARN] «bwrap» no puede crear el espacio de nombres en este sistema"
    echo "[WARN] suele ser la restricción de AppArmor; se habilita con:"
    echo "[WARN]     sudo sysctl -w kernel.apparmor_restrict_unprivileged_userns=0"
    echo "[WARN] se omite la prueba de extremo a extremo: sin aislamiento real"
    echo "[WARN] escribiría en tu ~/.ssh, así que no se ejecuta"
    exit 0
fi

if [ ! -x "$BASE/target/debug/yonder" ]; then
    echo "[ERR] falta target/debug/yonder; ejecuta «cargo build» primero"
    exit 1
fi

# El directorio de inicio real es también donde suele vivir el proyecto, y el
# montaje lo taparía: los binarios, las fuentes y el propio guion se copian
# fuera antes de entrar.
mkdir -p "$HOGAR_FALSO" "$BINARIOS" "$TRABAJO/trabajo"
cp "$BASE/target/debug/yonder" "$BASE/target/debug/yonder-askpass" "$BINARIOS/"
cp -r "$BASE/src" "$TRABAJO/fuentes"
cp "$BASE/pruebas/e2e.sh" "$TRABAJO/e2e.sh"
chmod +x "$TRABAJO/e2e.sh"

HOGAR_REAL="$(getent passwd "$(id -u)" | cut -d: -f6)"
echo "[INFO] se sustituye «$HOGAR_REAL» por «$HOGAR_FALSO» dentro de la prueba"

limpiar() { rm -rf "$TRABAJO"; }
trap limpiar EXIT

# /etc/ssh/ssh_config.d se enmascara: dentro del espacio de nombres de usuario
# los ficheros de root se ven con otro propietario y `ssh` se niega a leerlos.
bwrap \
    --dev-bind / / \
    --bind "$HOGAR_FALSO" "$HOGAR_REAL" \
    --tmpfs /etc/ssh/ssh_config.d \
    --die-with-parent \
    --setenv HOME "$HOGAR_REAL" \
    --setenv YONDER_BIN "$BINARIOS" \
    --setenv YONDER_FUENTES "$TRABAJO/fuentes" \
    --setenv YONDER_TRABAJO "$TRABAJO/trabajo" \
    "$TRABAJO/e2e.sh"
