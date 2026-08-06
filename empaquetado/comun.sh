#!/bin/bash
# Prepara el árbol de ficheros que comparten el .deb y el .rpm.
#
# Va aparte para que las dos recetas instalen exactamente lo mismo en los mismos
# sitios. Dos listas de ficheros que hay que mantener sincronizadas a mano
# acaban divergiendo, y el fallo se descubre cuando alguien instala el paquete
# de la distribución que menos usas.
#
# No se usa `cargo-deb` ni `cargo-generate-rpm` a propósito: son dos
# dependencias de compilación más que versionar y actualizar, para generar algo
# que `dpkg-deb` y `rpmbuild` ya hacen. Esto es todo el empaquetado, y cabe en
# una pantalla.

set -euo pipefail

RAIZ="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"

# Versión y descripción salen de Cargo.toml: una sola fuente de verdad.
version_del_proyecto() {
    grep -m1 '^version *= *"' "$RAIZ/Cargo.toml" | sed 's/.*"\(.*\)".*/\1/'
}

descripcion_del_proyecto() {
    grep -m1 '^description *= *"' "$RAIZ/Cargo.toml" | sed 's/.*"\(.*\)".*/\1/'
}

# ORDEN DEL CICLO DE PUBLICACIÓN
#
#   1. subir la versión en Cargo.toml
#   2. compilar
#   3. REGENERAR LAS CAPTURAS del README
#   4. pruebas, commit, etiqueta, empaquetado
#
# El paso 3 va después del 1 y no antes: la barra de estado muestra la versión
# en ejecución, así que una captura tomada antes de subirla enseña la anterior y
# el README de cada versión acaba ilustrado con la de al lado.

# Comprueba que los binarios están compilados en modo release.
exigir_binarios() {
    local faltan=0
    for binario in yonder yonder-askpass; do
        if [ ! -x "$RAIZ/target/release/$binario" ]; then
            echo "[ERR] falta target/release/$binario" >&2
            faltan=1
        fi
    done
    if [ "$faltan" -eq 1 ]; then
        echo "[ERR] compila primero:  cargo build --release" >&2
        exit 1
    fi
}

# Rellena $1 con la jerarquía que se instala, relativa a la raíz del sistema.
#
#   /usr/bin/yonder
#   /usr/bin/yonder-askpass
#   /usr/share/applications/yonder.desktop
#   /usr/share/icons/hicolor/scalable/apps/yonder.svg
#   /usr/share/doc/yonder/{README.md,LICENSE,ejemplo.conf}
poblar_arbol() {
    local destino="$1"

    install -Dm755 "$RAIZ/target/release/yonder"          "$destino/usr/bin/yonder"
    install -Dm755 "$RAIZ/target/release/yonder-askpass"  "$destino/usr/bin/yonder-askpass"

    install -Dm644 "$RAIZ/empaquetado/yonder.desktop" \
        "$destino/usr/share/applications/yonder.desktop"
    install -Dm644 "$RAIZ/assets/yonder.svg" \
        "$destino/usr/share/icons/hicolor/scalable/apps/yonder.svg"

    install -Dm644 "$RAIZ/README.md"  "$destino/usr/share/doc/yonder/README.md"
    install -Dm644 "$RAIZ/LICENSE"    "$destino/usr/share/doc/yonder/LICENSE"
    install -Dm644 "$RAIZ/docs/ejemplo.conf" \
        "$destino/usr/share/doc/yonder/ejemplo.conf"
    install -Dm644 "$RAIZ/docs/example.conf" \
        "$destino/usr/share/doc/yonder/example.conf"
}

# Texto largo de la descripción, común a los dos formatos.
#
# Menciona explícitamente lo de los puertos privilegiados: el paquete NO hace
# `setcap` por su cuenta. Conceder una capacidad a un binario es una decisión
# de seguridad de quien administra la máquina, no algo que deba pasar por
# instalar un paquete sin que nadie lo pida.
cuerpo_descripcion() {
    cat <<'TEXTO'
Herramienta gráfica y de línea de órdenes para definir, activar y supervisar
túneles SSH. No reimplementa SSH: el binario ssh del sistema es el motor, y
esto es un orquestador y un panel de control.

Los túneles se guardan en ~/.ssh/config.d/yonder.conf, así que funcionan
también con ssh, scp, rsync y VS Code Remote sin la aplicación abierta.

Detecta el "túnel zombi": el puerto local abierto que no transporta nada.

Para reenviar puertos por debajo de 1024 hace falta conceder una capacidad al
binario; el paquete no lo hace solo:
    sudo setcap 'cap_net_bind_service=+ep' /usr/bin/yonder
TEXTO
}
