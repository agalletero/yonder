//! `yonder-askpass` — binario auxiliar de §5.1.
//!
//! Contrato con OpenSSH:
//!
//! - Recibe el prompt como primer argumento.
//! - Escribe la respuesta por la salida estándar.
//! - Sale con 0 si hay respuesta y con distinto de 0 si el usuario cancela.
//!
//! `ssh` lo invoca solo cuando `SSH_ASKPASS` apunta aquí. Es tan simple como
//! parece, y esa es toda la intención: por aquí pasan contraseñas, así que
//! tiene que caber entero en la cabeza de quien lo audite.

use std::io::Write;
use std::process::ExitCode;

use yonder::askpass::cliente;
use yonder::rutas;

fn main() -> ExitCode {
    let prompt = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "Contraseña:".to_string());

    let socket = match rutas::socket_askpass() {
        Ok(ruta) => ruta,
        Err(e) => {
            eprintln!("[ERR] no se pudo determinar la ruta del socket: {e}");
            return ExitCode::FAILURE;
        }
    };

    match cliente::preguntar(&socket, &prompt) {
        Ok(respuesta) => match respuesta.texto {
            Some(texto) => {
                // Sin salto de línea propio: `ssh` recorta el final, pero así
                // se respeta literalmente lo que el usuario escribió.
                let mut salida = std::io::stdout();
                if salida.write_all(texto.as_bytes()).is_err() {
                    return ExitCode::FAILURE;
                }
                let _ = salida.write_all(b"\n");
                let _ = salida.flush();
                ExitCode::SUCCESS
            }
            None => {
                eprintln!("[INFO] cancelado por el usuario");
                ExitCode::FAILURE
            }
        },
        Err(e) => {
            eprintln!("[ERR] {e}");
            ExitCode::FAILURE
        }
    }
}
