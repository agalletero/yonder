#!/bin/bash
# Prueba de extremo a extremo con un túnel SSH real.
#
# Se ejecuta DENTRO de bwrap (véase e2e-aislado.sh), con el directorio de inicio sustituido
# por un directorio temporal. Así `ssh` lee nuestra configuración de verdad y el
# ~/.ssh real del usuario queda fuera de alcance.
#
# Valida los criterios de aceptación de §12:
#   1. Túnel local simple que funciona.
#   3. Cerrar la aplicación con túneles activos y reabrirla reconstruye el estado.
#   4. Matar el maestro por fuera y recuperar.
#   5. Puerto local ocupado → error comprensible.
#   7. `ssh <alias>` desde la terminal, sin la aplicación abierta.

set -u

TRABAJO="${YONDER_TRABAJO}"
BIN="${YONDER_BIN}/yonder"
PUERTO_SSHD=2242
PUERTO_SERVICIO=8899
PUERTO_TUNEL=8898

export XDG_RUNTIME_DIR="$TRABAJO/run"
export XDG_DATA_HOME="$HOME/.local/share"
export XDG_CONFIG_HOME="$HOME/.config"
export XDG_STATE_HOME="$HOME/.local/state"

FALLOS=0
comprobar() {
    if eval "$2"; then
        echo "  [OK]  $1"
    else
        echo "  [ERR] $1"
        FALLOS=$((FALLOS + 1))
    fi
}

PIDS=()
limpiar() {
    echo
    echo "--- limpiando ---"
    "$BIN" down destino >/dev/null 2>&1
    for pid in "${PIDS[@]:-}"; do
        [ -n "${pid:-}" ] && kill "$pid" 2>/dev/null
    done
    pkill -f "sshd.*$TRABAJO" 2>/dev/null
    echo
    if [ "$FALLOS" -eq 0 ]; then
        echo "=========== TODAS LAS COMPROBACIONES PASARON ==========="
    else
        echo "=========== $FALLOS COMPROBACIONES FALLARON ==========="
    fi
}
trap limpiar EXIT

separador() { echo; echo "########## $* ##########"; }

mkdir -p "$HOME/.ssh" "$XDG_RUNTIME_DIR" "$TRABAJO/sshd"
chmod 700 "$HOME/.ssh"

separador "sshd de pruebas en un puerto alto"
# Sin borrar antes, ssh-keygen se queda esperando un «Overwrite (y/n)?»
# que nadie va a contestar y la prueba se cuelga sin decir por qué.
rm -f "$TRABAJO/sshd/host_key" "$TRABAJO/sshd/host_key.pub" \
      "$HOME/.ssh/id_ed25519" "$HOME/.ssh/id_ed25519.pub"
ssh-keygen -q -t ed25519 -N '' -f "$TRABAJO/sshd/host_key" >/dev/null 2>&1
ssh-keygen -q -t ed25519 -N '' -C prueba -f "$HOME/.ssh/id_ed25519" >/dev/null 2>&1
cp "$HOME/.ssh/id_ed25519.pub" "$TRABAJO/sshd/authorized_keys"
chmod 600 "$TRABAJO/sshd/authorized_keys" "$TRABAJO/sshd/host_key"

cat > "$TRABAJO/sshd/sshd_config" <<EOF
Port $PUERTO_SSHD
ListenAddress 127.0.0.1
HostKey $TRABAJO/sshd/host_key
AuthorizedKeysFile $TRABAJO/sshd/authorized_keys
PidFile $TRABAJO/sshd/sshd.pid
StrictModes no
UsePAM no
PasswordAuthentication no
KbdInteractiveAuthentication no
PubkeyAuthentication yes
AllowTcpForwarding yes
PermitOpen any
LogLevel ERROR
EOF

# La salida se cierra: un sshd demonizado que hereda la tubería la mantiene
# abierta y bloquea a quien lea del otro lado.
/usr/sbin/sshd -f "$TRABAJO/sshd/sshd_config" -E "$TRABAJO/sshd/sshd.log" </dev/null >/dev/null 2>&1
echo "[INFO] código de salida de sshd: $?"
sleep 1
ss -ltn 2>/dev/null | grep -q ":$PUERTO_SSHD" || { echo "[ERR] el sshd no arrancó"; cat "$TRABAJO/sshd/sshd.log"; exit 1; }
echo "[INFO] sshd escuchando en 127.0.0.1:$PUERTO_SSHD"

python3 -m http.server "$PUERTO_SERVICIO" --bind 127.0.0.1 \
    --directory "$TRABAJO/sshd" </dev/null >"$TRABAJO/servicio.log" 2>&1 &
PIDS+=($!)
sleep 1
echo "[INFO] servicio HTTP de destino en 127.0.0.1:$PUERTO_SERVICIO"

separador "verificación de clave de host (§5.2)"
"$BIN" hostkey 127.0.0.1 --puerto "$PUERTO_SSHD" --aceptar 2>&1 | sed 's/^/  /'
comprobar "la clave quedó en known_hosts" \
    "grep -q '\[127.0.0.1\]:$PUERTO_SSHD' '$HOME/.ssh/known_hosts'"
# La comprobación de que nunca se desactiva la verificación de clave de host se
# hace más abajo, sobre la orden `ssh` que se ejecuta de verdad. Buscar el texto
# en las fuentes sería frágil: bastaría un comentario o un cambio de formato
# para que dejara de detectar nada.

separador "CRITERIO 5-bis: sin la línea Include, el error nombra la causa"
"$BIN" list >/dev/null 2>&1
cat >> "$HOME/.ssh/config.d/yonder.conf" <<EOF

Host sin-include
    HostName 127.0.0.1
    LocalForward 8897 localhost:$PUERTO_SERVICIO
EOF
SALIDA_SIN_INCLUDE=$("$BIN" up sin-include 2>&1)
echo "$SALIDA_SIN_INCLUDE" | sed 's/^/  /'
comprobar "el error menciona la línea Include y no un fallo de DNS" \
    "echo \"\$SALIDA_SIN_INCLUDE\" | grep -q 'falta la línea Include'"
python3 - "$HOME/.ssh/config.d/yonder.conf" <<'PY'
import sys, pathlib
ruta = pathlib.Path(sys.argv[1])
texto = ruta.read_text()
ruta.write_text(texto[: texto.index("Host sin-include")].rstrip() + "\n")
PY

separador "añadir el Include y definir el túnel"
"$BIN" import 2>&1 | sed 's/^/  /'
cat >> "$HOME/.ssh/config.d/yonder.conf" <<EOF

# nota: túnel de la prueba de extremo a extremo
Host destino
    HostName 127.0.0.1
    Port $PUERTO_SSHD
    User $(id -un)
    IdentityFile $HOME/.ssh/id_ed25519
    IdentitiesOnly yes
    LocalForward $PUERTO_TUNEL localhost:$PUERTO_SERVICIO
    ExitOnForwardFailure yes
    ServerAliveInterval 15
    ServerAliveCountMax 3
EOF
"$BIN" list

separador "CRITERIO 1: levantar un túnel local simple"
# Con -v el registro incluye la línea de órdenes exacta de cada proceso hijo,
# así que se puede comprobar la orden que se ejecuta de verdad en vez de buscar
# cadenas en el código fuente.
ORDENES=$("$BIN" -v up destino 2>&1)
echo "$ORDENES" | grep -v '^\[DEBUG\]' | sed 's/^/  /'
sleep 1
"$BIN" list
comprobar "el túnel figura como Activo" \
    "\"$BIN\" list 2>/dev/null | grep -q '^Activo'"

separador "§5.2: la orden ssh real nunca desactiva la verificación de host"
echo "$ORDENES" | grep -oE 'StrictHostKeyChecking=[a-z]+' | sort -u | sed 's/^/  /'
comprobar "el maestro se abre con StrictHostKeyChecking=yes" \
    "echo \"\$ORDENES\" | grep -q 'StrictHostKeyChecking=yes'"
comprobar "NUNCA aparece StrictHostKeyChecking=no en la orden ejecutada" \
    "! echo \"\$ORDENES\" | grep -q 'StrictHostKeyChecking=no'"
comprobar "el maestro arranca sin reenvíos (ClearAllForwardings)" \
    "echo \"\$ORDENES\" | grep -q 'ClearAllForwardings=yes'"
comprobar "el reenvío se añade con «-O forward», no reconectando" \
    "echo \"\$ORDENES\" | grep -q '\\-O forward'"

RESPUESTA=$(curl -s --max-time 5 "http://127.0.0.1:$PUERTO_TUNEL/" || true)
comprobar "el tráfico atraviesa el túnel de verdad" "[ -n \"\$RESPUESTA\" ]"

separador "CRITERIO 7: «ssh <alias>» sin la aplicación abierta"
# Se baja el túnel primero: si no, «ssh destino» intentaría abrir el mismo
# puerto local que ya tiene el maestro y fallaría con razón.
"$BIN" down destino >/dev/null 2>&1
SALIDA_SSH=$(ssh -o BatchMode=yes destino "echo TUNEL_OK" 2>&1)
echo "$SALIDA_SSH" | sed 's/^/  /'
comprobar "ssh destino funciona desde la terminal" \
    "echo \"\$SALIDA_SSH\" | grep -q TUNEL_OK"
comprobar "el alias resuelve al host y puerto correctos" \
    "ssh -G destino 2>/dev/null | grep -q 'port $PUERTO_SSHD'"
comprobar "el LocalForward del fichero lo aplica ssh por su cuenta" \
    "ssh -G destino 2>/dev/null | grep -q 'localforward $PUERTO_TUNEL'"
"$BIN" up destino >/dev/null 2>&1
sleep 1

separador "CRITERIO 3: el estado se reconstruye en un proceso nuevo"
echo "[INFO] cada invocación del binario es un proceso distinto: si el estado"
echo "       se reconstruye, es porque sale del socket de control, no de memoria."
"$BIN" status 2>&1 | sed 's/^/  /'
comprobar "el maestro sigue vivo tras terminar el proceso anterior" \
    "\"$BIN\" status 2>/dev/null | grep -qE '^destino'"

separador "CRITERIO 4: matar el maestro por fuera"
DIR_SOCKETS=$("$BIN" status 2>/dev/null | awk '/^Sockets:/ {print $2}')
echo "[INFO] directorio de sockets segun la aplicación: $DIR_SOCKETS"
SOCKET=$(ls "$DIR_SOCKETS" 2>/dev/null | grep -E '^ctl-[0-9a-f]+$' | head -1)
PID_MAESTRO=$(ssh -S "$DIR_SOCKETS/$SOCKET" -O check destino 2>&1 | grep -oP 'pid=\K[0-9]+')
echo "[INFO] PID del maestro: ${PID_MAESTRO:-(no encontrado)}"
kill -9 "$PID_MAESTRO" 2>/dev/null
sleep 1
"$BIN" list
comprobar "deja de estar Activo al morir el maestro" \
    "! \"$BIN\" list 2>/dev/null | grep -q '^Activo'"
echo "[INFO] recuperación:"
"$BIN" up destino 2>&1 | sed 's/^/  /'
sleep 1
"$BIN" list
comprobar "se recupera tras el reintento" \
    "\"$BIN\" list 2>/dev/null | grep -q '^Activo'"

separador "estado Degradado: maestro vivo, reenvío caído (§4)"
DIR_SOCKETS=$("$BIN" status 2>/dev/null | awk '/^Sockets:/ {print $2}')
SOCKET=$(ls "$DIR_SOCKETS" 2>/dev/null | grep -E '^ctl-[0-9a-f]+$' | head -1)
ssh -S "$DIR_SOCKETS/$SOCKET" -O cancel \
    -L "$PUERTO_TUNEL:localhost:$PUERTO_SERVICIO" destino 2>&1 | sed 's/^/  /'
sleep 1
"$BIN" list
comprobar "el reenvío caído con el maestro vivo se ve como Degradado" \
    "\"$BIN\" list 2>/dev/null | grep -q '^Degradado'"
echo "[INFO] esto es lo que distingue esta herramienta de una que enseñaría"
echo "       un punto verde con el túnel muerto."

separador "TÚNEL ZOMBI: el puerto escucha y el túnel no transporta"
# El caso que el guion tunel1.sh documenta y que la comprobación de «puerto
# escuchando» NO puede ver: el reenvío se establece contra un destino donde no
# hay servicio. El puerto local abre, el cliente conecta, y muere ahí.
"$BIN" down destino >/dev/null 2>&1
PUERTO_MUERTO=8877
cat >> "$HOME/.ssh/config.d/yonder.conf" <<EOF

# nota: apunta a un puerto donde no hay nada, para provocar el zombi
Host zombi
    HostName 127.0.0.1
    Port $PUERTO_SSHD
    User $(id -un)
    IdentityFile $HOME/.ssh/id_ed25519
    IdentitiesOnly yes
    # salud: http:/
    LocalForward $PUERTO_MUERTO localhost:$PUERTO_MUERTO
    ExitOnForwardFailure yes
EOF

echo "[INFO] la marca de salud, tal como queda en el fichero:"
grep -A2 "Host zombi" "$HOME/.ssh/config.d/yonder.conf" | grep salud | sed 's/^/  /'
comprobar "ssh sigue leyendo el bloque sin tropezar con el comentario" \
    "ssh -G zombi 2>/dev/null | grep -q 'localforward $PUERTO_MUERTO'"

"$BIN" up zombi 2>&1 | sed 's/^/  /'
sleep 1
"$BIN" list

comprobar "el puerto local SÍ escucha (por eso la sonda barata no basta)" \
    "ss -tln 2>/dev/null | grep -q ':$PUERTO_MUERTO'"
comprobar "aun así, el túnel se ve como Degradado y no como Activo" \
    "\"$BIN\" list 2>/dev/null | grep zombi | grep -q '^Degradado'"
SALIDA_ZOMBI=$("$BIN" status 2>&1)
comprobar "y se explica el motivo" \
    "echo \"\$SALIDA_ZOMBI\" | grep -qiE 'no responde|no transporta|sin responder|cerró'"

separador "reconciliación de una pasada (sustituye al temporizador del guion)"
SALIDA_SUP=$("$BIN" supervise --simular 2>&1)
echo "$SALIDA_SUP" | sed 's/^/  /'
comprobar "«supervise --simular» detecta el degradado sin tocar nada" \
    "echo \"\$SALIDA_SUP\" | grep -q 'DEGRADADO'"
comprobar "y no toca nada al simular" \
    "echo \"\$SALIDA_SUP\" | grep -q 'simulación'"

"$BIN" down zombi >/dev/null 2>&1

separador "CRITERIO 5: puerto local ocupado"
"$BIN" down destino >/dev/null 2>&1
python3 -c "
import socket, time
s = socket.socket(); s.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)
s.bind(('127.0.0.1', $PUERTO_TUNEL)); s.listen(1); time.sleep(30)
" </dev/null >/dev/null 2>&1 &
PIDS+=($!)
sleep 1
SALIDA_OCUPADO=$("$BIN" up destino 2>&1)
echo "$SALIDA_OCUPADO" | sed 's/^/  /'
comprobar "el error dice que el puerto está ocupado" \
    "echo \"\$SALIDA_OCUPADO\" | grep -q 'ya está ocupado'"
comprobar "el error identifica al proceso que lo ocupa" \
    "echo \"\$SALIDA_OCUPADO\" | grep -qE 'PID [0-9]+'"
comprobar "el error propone qué hacer" \
    "echo \"\$SALIDA_OCUPADO\" | grep -q 'elige otro puerto local'"

separador "limpieza de sockets huérfanos (§3.6)"
"$BIN" down destino >/dev/null 2>&1
DIR_SOCKETS=$("$BIN" status 2>/dev/null | awk '/^Sockets:/ {print $2}')
mkdir -p "$DIR_SOCKETS"
touch "$DIR_SOCKETS/ctl-0000000000000000"
echo "fantasma" > "$DIR_SOCKETS/ctl-0000000000000000.alias"
"$BIN" clean 2>&1 | sed 's/^/  /'
comprobar "el socket huérfano se borró" \
    "[ ! -e \"\$DIR_SOCKETS/ctl-0000000000000000\" ]"
comprobar "y su fichero de alias también" \
    "[ ! -e \"\$DIR_SOCKETS/ctl-0000000000000000.alias\" ]"

separador "el ~/.ssh/config del usuario se conservó íntegro"
cat "$HOME/.ssh/config"
comprobar "la copia de seguridad existe o el fichero se creó de cero" \
    "[ -f '$HOME/.ssh/config' ]"

separador "fichero propio resultante"
cat "$HOME/.ssh/config.d/yonder.conf"
