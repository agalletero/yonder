//! Registro de eventos (§11).
//!
//! Sin colores y con prefijos `[ERR]`, `[WARN]`, `[INFO]`, `[DEBUG]`. Los
//! colores en la salida de una herramienta que se va a redirigir a un fichero o
//! a `grep` son ruido, y el prefijo se lee igual de bien en los dos sitios.

use std::fmt;
use std::fs::File;
use std::io::Write;
use std::sync::{Arc, Mutex};

use tracing::{Event, Level, Subscriber};
use tracing_subscriber::fmt::format::Writer;
use tracing_subscriber::fmt::{FmtContext, FormatEvent, FormatFields, MakeWriter};
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::registry::LookupSpan;
use tracing_subscriber::util::SubscriberInitExt;
use tracing_subscriber::EnvFilter;

use crate::rutas;

/// Formato `[NIVEL] mensaje campo=valor`.
struct FormatoPrefijado;

impl<S, N> FormatEvent<S, N> for FormatoPrefijado
where
    S: Subscriber + for<'a> LookupSpan<'a>,
    N: for<'a> FormatFields<'a> + 'static,
{
    fn format_event(
        &self,
        contexto: &FmtContext<'_, S, N>,
        mut escritor: Writer<'_>,
        evento: &Event<'_>,
    ) -> fmt::Result {
        let etiqueta = match *evento.metadata().level() {
            Level::ERROR => "ERR",
            Level::WARN => "WARN",
            Level::INFO => "INFO",
            Level::DEBUG => "DEBUG",
            Level::TRACE => "TRACE",
        };
        write!(escritor, "[{etiqueta}] ")?;
        contexto
            .field_format()
            .format_fields(escritor.by_ref(), evento)?;
        writeln!(escritor)
    }
}

/// Escritor al fichero de registro, compartido entre hilos.
#[derive(Clone)]
struct FicheroCompartido(Arc<Mutex<File>>);

impl Write for FicheroCompartido {
    fn write(&mut self, datos: &[u8]) -> std::io::Result<usize> {
        match self.0.lock() {
            Ok(mut fichero) => fichero.write(datos),
            // Un registro que no se puede escribir no debe tumbar la aplicación.
            Err(_) => Ok(datos.len()),
        }
    }

    fn flush(&mut self) -> std::io::Result<()> {
        match self.0.lock() {
            Ok(mut fichero) => fichero.flush(),
            Err(_) => Ok(()),
        }
    }
}

impl<'a> MakeWriter<'a> for FicheroCompartido {
    type Writer = FicheroCompartido;

    fn make_writer(&'a self) -> Self::Writer {
        self.clone()
    }
}

/// Inicializa el registro.
///
/// - Siempre a la salida de error, con el formato prefijado.
/// - Además al fichero de §6 si se puede abrir; si no, se avisa y se sigue.
/// - `RUST_LOG` manda sobre el nivel por defecto.
pub fn iniciar(verboso: bool) {
    let por_defecto = if verboso { "debug" } else { "info" };
    let filtro = EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| EnvFilter::new(format!("yonder={por_defecto},warn")));

    let terminal = tracing_subscriber::fmt::layer()
        .event_format(FormatoPrefijado)
        .with_ansi(false)
        .with_writer(std::io::stderr);

    let registro = tracing_subscriber::registry().with(filtro).with(terminal);

    match abrir_fichero() {
        Some(fichero) => {
            let capa = tracing_subscriber::fmt::layer()
                .event_format(FormatoPrefijado)
                .with_ansi(false)
                .with_writer(fichero);
            let _ = registro.with(capa).try_init();
        }
        None => {
            let _ = registro.try_init();
        }
    }
}

fn abrir_fichero() -> Option<FicheroCompartido> {
    let ruta = rutas::registro().ok()?;
    let directorio = ruta.parent()?;
    rutas::asegurar_directorio_privado(directorio).ok()?;

    use std::os::unix::fs::OpenOptionsExt;
    let fichero = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .mode(0o600)
        .open(&ruta)
        .ok()?;
    Some(FicheroCompartido(Arc::new(Mutex::new(fichero))))
}

/// Salida a consola con las mismas etiquetas, para lo que no es un evento de
/// registro sino una respuesta al usuario.
pub mod consola {
    pub fn info(mensaje: impl AsRef<str>) {
        println!("[INFO] {}", mensaje.as_ref());
    }

    pub fn aviso(mensaje: impl AsRef<str>) {
        eprintln!("[WARN] {}", mensaje.as_ref());
    }

    pub fn error(mensaje: impl AsRef<str>) {
        eprintln!("[ERR] {}", mensaje.as_ref());
    }

    /// Texto sin prefijo: tablas, cabeceras y todo lo que se va a leer en
    /// bloque. Un prefijo por línea en una tabla la haría ilegible.
    pub fn texto(mensaje: impl AsRef<str>) {
        println!("{}", mensaje.as_ref());
    }
}
