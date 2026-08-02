#!/bin/bash
# Construye el paquete .rpm con rpmbuild, sin más herramientas.
#
# Uso:
#   cargo build --release
#   empaquetado/construir-rpm.sh            # deja el .rpm en dist/
#   empaquetado/construir-rpm.sh <destino>
#
# En Debian y derivadas: sudo apt install rpm
#
# Las dependencias se declaran por **soname** y no por nombre de paquete:
# `libGL.so.1()(64bit)` lo satisface mesa-libGL en Fedora, Mesa-libGL1 en
# openSUSE y libgl1 en Mageia sin que aquí haya que saberlo. Con nombres de
# paquete habría que mantener una tabla por distribución, que es justo la clase
# de trabajo que no queremos.

set -euo pipefail

RAIZ="$(cd "$(dirname "$0")/.." && pwd)"
# shellcheck source=empaquetado/comun.sh
source "$RAIZ/empaquetado/comun.sh"

DESTINO="${1:-$RAIZ/dist}"
VERSION="$(version_del_proyecto)"
ARQUITECTURA="$(uname -m)"

exigir_binarios

if ! command -v rpmbuild >/dev/null 2>&1; then
    echo "[ERR] falta rpmbuild. En Debian y derivadas:  sudo apt install rpm" >&2
    exit 1
fi

TRABAJO="$(mktemp -d)"
trap 'rm -rf "$TRABAJO"' EXIT

RAIZ_INSTALACION="$TRABAJO/raiz"
poblar_arbol "$RAIZ_INSTALACION"

mkdir -p "$TRABAJO/rpmbuild/"{BUILD,RPMS,SOURCES,SPECS,SRPMS}
ESPECIFICACION="$TRABAJO/rpmbuild/SPECS/yonder.spec"

{
    cat <<ESPEC
Name:           yonder
Version:        $VERSION
Release:        1
Summary:        $(descripcion_del_proyecto)
License:        MIT
URL:            https://github.com/agalletero/yonder
BuildArch:      $ARQUITECTURA

# El binario ya viene compilado: aquí solo se empaqueta.
%global debug_package %{nil}
%global __strip /bin/true

Requires:       openssh-clients
Requires:       libGL.so.1()(64bit)
Requires:       libX11.so.6()(64bit)
Requires:       libxkbcommon.so.0()(64bit)
Requires:       libxkbcommon-x11.so.0()(64bit)
Requires:       libwayland-client.so.0()(64bit)
Requires:       libwayland-egl.so.1()(64bit)

%description
ESPEC
    cuerpo_descripcion

    cat <<'ESPEC'

%install
cp -a %{_sourcedir}/raiz/. %{buildroot}/

%files
%{_bindir}/yonder
%{_bindir}/yonder-askpass
%{_datadir}/applications/yonder.desktop
%{_datadir}/icons/hicolor/scalable/apps/yonder.svg
%dir %{_datadir}/doc/yonder
%doc %{_datadir}/doc/yonder/README.md
%doc %{_datadir}/doc/yonder/ejemplo.conf
%license %{_datadir}/doc/yonder/LICENSE

%changelog
ESPEC
} > "$ESPECIFICACION"

# `openssh-clients` es el nombre en Fedora y RHEL; en openSUSE es `openssh`.
# Si la distribución de destino no lo tiene, se puede construir sin él:
#   SIN_OPENSSH=1 empaquetado/construir-rpm.sh
if [ "${SIN_OPENSSH:-0}" = "1" ]; then
    sed -i '/^Requires: *openssh-clients$/d' "$ESPECIFICACION"
    echo "[WARN] se omite la dependencia de openssh-clients a petición"
fi

rpmbuild \
    --define "_topdir $TRABAJO/rpmbuild" \
    --define "_sourcedir $RAIZ_INSTALACION/.." \
    --buildroot "$TRABAJO/buildroot" \
    -bb "$ESPECIFICACION" >"$TRABAJO/rpmbuild.log" 2>&1 || {
        echo "[ERR] rpmbuild falló:" >&2
        tail -30 "$TRABAJO/rpmbuild.log" >&2
        exit 1
    }

mkdir -p "$DESTINO"
find "$TRABAJO/rpmbuild/RPMS" -name '*.rpm' -exec cp {} "$DESTINO/" \;

for paquete in "$DESTINO"/yonder-"$VERSION"-*.rpm; do
    echo "[INFO] $paquete"
    rpm -qip "$paquete" | sed 's/^/  /'
    echo "[INFO] contenido:"
    rpm -qlp "$paquete" | sed 's/^/  /'
    echo "[INFO] dependencias declaradas:"
    rpm -qRp "$paquete" | sed 's/^/  /'
done
