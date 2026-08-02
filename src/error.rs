//! Errores de la biblioteca.
//!
//! Convención §11: `thiserror` en las capas de biblioteca, `anyhow` en el
//! binario. Todos los mensajes en español y orientados a que la interfaz los
//! pueda mostrar tal cual, sin reescribirlos.

use std::path::PathBuf;

use thiserror::Error;

pub type Resultado<T> = std::result::Result<T, Error>;

#[derive(Debug, Error)]
pub enum Error {
    #[error("no se pudo leer «{ruta}»: {origen}")]
    Lectura {
        ruta: PathBuf,
        #[source]
        origen: std::io::Error,
    },

    #[error("no se pudo escribir «{ruta}»: {origen}")]
    Escritura {
        ruta: PathBuf,
        #[source]
        origen: std::io::Error,
    },

    #[error("error de entrada/salida: {0}")]
    Io(#[from] std::io::Error),

    #[error("«{ruta}»:{linea}: {motivo}")]
    Sintaxis {
        ruta: PathBuf,
        linea: usize,
        motivo: String,
    },

    #[error("no existe el túnel «{0}»")]
    TunelDesconocido(String),

    #[error("no existe el host «{0}» en la configuración")]
    HostDesconocido(String),

    #[error("ya existe un host llamado «{0}»")]
    HostDuplicado(String),

    #[error(
        "«{0}» no lo gestiona esta aplicación: está definido en otro fichero y es de solo lectura"
    )]
    HostAjeno(String),

    #[error("no se encontró el ejecutable «{0}» en el PATH")]
    EjecutableAusente(String),

    #[error("«{orden}» agotó el tiempo de espera ({segundos} s)")]
    TiempoAgotado { orden: String, segundos: u64 },

    #[error("«{orden}» terminó con código {codigo}: {salida}")]
    OrdenFallida {
        orden: String,
        codigo: i32,
        salida: String,
    },

    #[error("el puerto local {puerto} ya está ocupado{}", detalle_ocupante(.ocupante))]
    PuertoOcupado {
        puerto: u16,
        ocupante: Option<String>,
    },

    #[error(
        "el puerto {puerto} es privilegiado (<1024) y el proceso no tiene CAP_NET_BIND_SERVICE"
    )]
    PuertoPrivilegiado { puerto: u16 },

    #[error("definición de túnel no válida: {0}")]
    DefinicionInvalida(String),

    #[error("el maestro SSH de «{0}» no está activo")]
    MaestroInactivo(String),

    #[error("error de base de datos: {0}")]
    BaseDeDatos(String),

    #[error("error del almacén de secretos: {0}")]
    Secretos(String),

    #[error("la operación fue cancelada por el usuario")]
    Cancelada,

    #[error("no se pudo determinar la ruta «{0}»; revisa las variables XDG del entorno")]
    RutaIndeterminada(&'static str),

    #[error(
        "«{alias}» está definido en el fichero de túneles, pero «ssh» no lo ve: falta la línea \
         Include en ~/.ssh/config"
    )]
    AliasInvisible { alias: String },
}

fn detalle_ocupante(ocupante: &Option<String>) -> String {
    match ocupante {
        Some(quien) => format!(" por {quien}"),
        None => String::new(),
    }
}

impl Error {
    /// Error de lectura con la ruta anotada, para no perder el contexto.
    pub fn leyendo(ruta: impl Into<PathBuf>, origen: std::io::Error) -> Self {
        Error::Lectura {
            ruta: ruta.into(),
            origen,
        }
    }

    /// Error de escritura con la ruta anotada.
    pub fn escribiendo(ruta: impl Into<PathBuf>, origen: std::io::Error) -> Self {
        Error::Escritura {
            ruta: ruta.into(),
            origen,
        }
    }

    /// Sugerencia accionable para la interfaz. `None` si no hay una obvia.
    ///
    /// Es lo que separa «falló» de «falló y esto es lo que puedes hacer», que
    /// es el objetivo del criterio de aceptación 5 (§12).
    pub fn sugerencia(&self) -> Option<String> {
        match self {
            Error::PuertoOcupado { puerto, ocupante } => Some(match ocupante {
                Some(quien) => {
                    format!("Cierra {quien} o elige otro puerto local distinto de {puerto}.")
                }
                None => format!("Elige otro puerto local distinto de {puerto}."),
            }),
            Error::PuertoPrivilegiado { .. } => Some(format!(
                "Concede la capacidad al binario:\n    sudo setcap 'cap_net_bind_service=+ep' {}\n\
                 …o usa un puerto local por encima de 1023.",
                std::env::current_exe()
                    .map(|r| r.display().to_string())
                    .unwrap_or_else(|_| "<PATH>/yonder".to_string())
            )),
            Error::EjecutableAusente(nombre) => Some(format!(
                "Instala el paquete de OpenSSH que proporciona «{nombre}»."
            )),
            Error::MaestroInactivo(alias) => {
                Some(format!("Levanta primero la conexión maestra de «{alias}»."))
            }
            Error::HostAjeno(_) => Some(
                "Impórtalo desde la pantalla de importación para poder gestionarlo.".to_string(),
            ),
            Error::AliasInvisible { .. } => Some(format!(
                "Añade esta línea al principio de ~/.ssh/config:\n    {}\n\
                 …o ejecuta «yonder import», que la pone con copia de seguridad previa.",
                crate::rutas::LINEA_INCLUDE
            )),
            _ => None,
        }
    }
}
