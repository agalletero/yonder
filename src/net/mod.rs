//! Sondeo de red local. Todo se resuelve con `/proc`, sin `lsof` (§5.3).

pub mod probe;
pub mod salud;

pub use probe::{
    comprobar_puerto_libre, esta_escuchando, orden_para_conceder_capacidad,
    puede_abrir_puertos_privilegiados, quien_ocupa, Escuchas, Ocupante,
};
pub use salud::{sondear, Veredicto};
