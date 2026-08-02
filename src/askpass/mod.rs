//! Askpass propio (§5.1) — la pieza más traicionera del diseño.
//!
//! `ssh` no pide las contraseñas por la entrada estándar sino por el TTY. En
//! una interfaz gráfica no hay TTY, así que no se puede «capturar la salida y
//! escribir en la entrada»: hay que darle a `ssh` un programa externo al que
//! preguntar, vía `SSH_ASKPASS` y `SSH_ASKPASS_REQUIRE=force`.
//!
//! El binario auxiliar es minúsculo a propósito: recibe el prompt como
//! argumento, se conecta por socket Unix a la aplicación, esta muestra el modal
//! y devuelve la respuesta por la salida estándar.
//!
//! Se implementa en la fase 2, **antes** que la GUI, porque es lo que puede
//! invalidar decisiones de diseño si se deja para el final.

pub mod cliente;
pub mod protocolo;
pub mod servidor;

pub use protocolo::{Peticion, Respuesta, Tipo};
pub use servidor::{localizar_binario, PeticionPendiente, Servidor};
