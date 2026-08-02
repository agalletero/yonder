//! Gestor de túneles SSH: orquestador sobre el binario `ssh` del sistema.
//!
//! La biblioteca contiene todo el motor. Los dos binarios (`yonder` y
//! `yonder-askpass`) son envoltorios finos: el primero decide entre CLI y GUI,
//! el segundo es deliberadamente trivial (§5.1).
//!
//! Principios que atraviesan el código (§2):
//!
//! 1. No reimplementar lo que OpenSSH ya hace: el motor es el binario `ssh`.
//! 2. Cero criptografía propia.
//! 3. Todo túnel definido aquí funciona con `ssh <alias>` desde la terminal.
//! 4. Sin estado propio duplicado: la verdad vive en el socket de control.
//! 5. La aplicación no es un demonio, es un panel que se engancha y desengancha.

pub mod askpass;
pub mod cli;
pub mod config;
pub mod error;
pub mod modelo;
pub mod net;
pub mod prefs;
pub mod registro;
pub mod rutas;
pub mod secrets;
pub mod servicio;
pub mod ssh;
pub mod state;

pub use error::{Error, Resultado};
