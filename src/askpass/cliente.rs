//! Lado cliente del askpass: lo que ejecuta `yonder-askpass` (§5.1).
//!
//! Deliberadamente trivial. Se conecta, envía el prompt, espera la respuesta y
//! la escribe por la salida estándar, que es de donde la lee `ssh`. No hay
//! estado, ni ficheros, ni reintentos, ni nada que auditar más allá de esto.

use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixStream;
use std::path::Path;
use std::time::Duration;

use crate::error::{Error, Resultado};

use super::protocolo::{self, Peticion, Respuesta};

/// Envía el prompt al servidor y espera la respuesta.
pub fn preguntar(socket: &Path, prompt: &str) -> Resultado<Respuesta> {
    let mut flujo = UnixStream::connect(socket).map_err(|e| {
        if e.kind() == std::io::ErrorKind::NotFound
            || e.kind() == std::io::ErrorKind::ConnectionRefused
        {
            Error::DefinicionInvalida(format!(
                "no hay ninguna ventana de «yonder» escuchando en {}",
                socket.display()
            ))
        } else {
            Error::Io(e)
        }
    })?;

    // Al otro lado hay una persona: el margen es generoso a propósito.
    flujo
        .set_read_timeout(Some(Duration::from_secs(
            protocolo::ESPERA_RESPUESTA_SEGUNDOS + 10,
        )))
        .map_err(Error::Io)?;

    let peticion = Peticion::nueva(prompt);
    let linea =
        serde_json::to_string(&peticion).map_err(|e| Error::DefinicionInvalida(e.to_string()))?;
    writeln!(flujo, "{linea}").map_err(Error::Io)?;
    flujo.flush().map_err(Error::Io)?;

    let mut lector = BufReader::new(flujo);
    let mut respuesta = String::new();
    lector.read_line(&mut respuesta).map_err(Error::Io)?;

    serde_json::from_str(respuesta.trim())
        .map_err(|e| Error::DefinicionInvalida(format!("respuesta ilegible: {e}")))
}
