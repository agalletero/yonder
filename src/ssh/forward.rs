//! Alta y baja de reenvíos en caliente: `-O forward` / `-O cancel` (§3.2).
//!
//! Es la operación que hace que la herramienta se sienta ágil: activar y
//! desactivar túneles sueltos sobre una conexión ya autenticada, sin volver a
//! teclear nada ni tocar la llave física.

use crate::error::Resultado;
use crate::modelo::Reenvio;
use crate::net::probe;

use super::control::Control;
use super::Entorno;

/// Activa un reenvío sobre un maestro ya abierto.
///
/// El puerto local se comprueba **antes** de pedírselo a `ssh`: así el error
/// dice qué proceso lo ocupa en vez de dejar un «cannot listen to port» pelado
/// (§5.3).
pub fn activar(control: &Control, reenvio: &Reenvio, entorno: &Entorno) -> Resultado<()> {
    if reenvio.tipo.escucha_en_local() {
        probe::comprobar_puerto_libre(
            reenvio.escucha.direccion_efectiva(),
            reenvio.escucha.puerto,
        )?;
    }
    control.activar_reenvio(reenvio, entorno)?;
    tracing::info!(
        alias = %control.alias,
        reenvio = %reenvio.descripcion(),
        "reenvío activado"
    );
    Ok(())
}

/// Cancela un reenvío sin cerrar la conexión maestra.
pub fn cancelar(control: &Control, reenvio: &Reenvio, entorno: &Entorno) -> Resultado<()> {
    control.cancelar_reenvio(reenvio, entorno)?;
    tracing::info!(
        alias = %control.alias,
        reenvio = %reenvio.descripcion(),
        "reenvío cancelado"
    );
    Ok(())
}

/// Comprueba que el reenvío está **de verdad** en pie.
///
/// Esta es la comprobación que separa `Activo` de `Degradado` (§4). Un maestro
/// que responde a `-O check` no garantiza nada sobre los reenvíos: el puerto
/// local puede haberse caído y la lista seguiría enseñando un punto verde.
///
/// Para los reenvíos remotos no hay nada que sondear desde aquí (el puerto
/// escucha en la otra máquina), así que se da por bueno lo que diga el maestro.
pub fn confirmado(reenvio: &Reenvio) -> bool {
    confirmado_en(reenvio, &probe::Escuchas::tomar())
}

/// Igual que [`confirmado`] pero sobre una fotografía ya tomada.
///
/// El supervisor comprueba todos los túneles en cada latido: leer `/proc` una
/// vez y consultarla N veces es lo que hace que el sondeo sea gratis.
pub fn confirmado_en(reenvio: &Reenvio, escuchas: &probe::Escuchas) -> bool {
    if !reenvio.tipo.escucha_en_local() {
        return true;
    }
    escuchas.contiene(reenvio.escucha.direccion_efectiva(), reenvio.escucha.puerto)
}

#[cfg(test)]
mod pruebas {
    use super::*;
    use crate::modelo::{Extremo, TipoReenvio};
    use std::net::TcpListener;

    fn reenvio_a(puerto: u16) -> Reenvio {
        Reenvio::local(
            Extremo::nuevo("127.0.0.1", puerto),
            Extremo::nuevo("interno", 80),
        )
    }

    #[test]
    fn un_reenvio_local_con_su_puerto_escuchando_esta_confirmado() {
        let escucha = TcpListener::bind("127.0.0.1:0").unwrap();
        let puerto = escucha.local_addr().unwrap().port();
        assert!(confirmado(&reenvio_a(puerto)));
        drop(escucha);
    }

    #[test]
    fn un_reenvio_local_sin_puerto_en_escucha_esta_degradado() {
        // Un puerto efímero recién liberado puede habérselo quedado otra prueba
        // que corra en paralelo. Se reintenta hasta dar con uno que siga libre
        // en el momento de mirar: la prueba comprueba la lógica, no gana una
        // carrera contra el resto de la suite.
        let mut puerto_libre = None;
        for _ in 0..25 {
            let escucha = TcpListener::bind("127.0.0.1:0").unwrap();
            let puerto = escucha.local_addr().unwrap().port();
            drop(escucha);
            if probe::comprobar_puerto_libre("127.0.0.1", puerto).is_ok() {
                puerto_libre = Some(puerto);
                break;
            }
        }
        let puerto = puerto_libre.expect("no se encontró ningún puerto libre en 25 intentos");
        assert!(
            !confirmado(&reenvio_a(puerto)),
            "el puerto {puerto} no escucha: el reenvío debe darse por degradado"
        );
    }

    #[test]
    fn un_reenvio_remoto_no_se_sondea_en_local() {
        let reenvio = Reenvio {
            tipo: TipoReenvio::Remoto,
            escucha: Extremo::solo_puerto(9000),
            destino: Some(Extremo::nuevo("localhost", 22)),
            salud: crate::modelo::Salud::Escucha,
            nombre: None,
        };
        // El puerto 9000 no escucha aquí y aun así debe darse por bueno: escucha
        // en la otra punta.
        assert!(confirmado(&reenvio));
    }
}
