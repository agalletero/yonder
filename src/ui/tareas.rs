//! Tareas de fondo de un solo disparo.
//!
//! `ssh-keyscan` y `ssh-copy-id` tardan segundos. Ejecutarlos en el hilo del
//! dibujado congelaría la ventana, y una ventana congelada es indistinguible de
//! una colgada. Se lanzan en un hilo aparte y la interfaz consulta el resultado
//! en cada fotograma sin bloquearse nunca.

use std::sync::mpsc::{self, Receiver, TryRecvError};

use eframe::egui;

use yonder::error::Resultado;

/// Una tarea en curso.
pub struct Tarea<T> {
    receptor: Receiver<Resultado<T>>,
    /// Descripción para el indicador de progreso.
    pub descripcion: String,
    terminada: bool,
}

impl<T: Send + 'static> Tarea<T> {
    /// Lanza el trabajo en un hilo y devuelve el asa.
    ///
    /// Se le pasa el contexto de egui para poder pedir un repintado cuando
    /// termine: sin eso el resultado se quedaría esperando al siguiente
    /// movimiento del ratón.
    pub fn lanzar(
        contexto: &egui::Context,
        descripcion: impl Into<String>,
        trabajo: impl FnOnce() -> Resultado<T> + Send + 'static,
    ) -> Tarea<T> {
        let (emisor, receptor) = mpsc::channel();
        let contexto = contexto.clone();
        std::thread::spawn(move || {
            let resultado = trabajo();
            let _ = emisor.send(resultado);
            contexto.request_repaint();
        });
        Tarea {
            receptor,
            descripcion: descripcion.into(),
            terminada: false,
        }
    }

    /// Devuelve el resultado si ya está listo. No bloquea.
    pub fn resultado(&mut self) -> Option<Resultado<T>> {
        if self.terminada {
            return None;
        }
        match self.receptor.try_recv() {
            Ok(resultado) => {
                self.terminada = true;
                Some(resultado)
            }
            Err(TryRecvError::Empty) => None,
            Err(TryRecvError::Disconnected) => {
                self.terminada = true;
                Some(Err(yonder::Error::Cancelada))
            }
        }
    }

    pub fn en_curso(&self) -> bool {
        !self.terminada
    }
}

#[cfg(test)]
mod pruebas {
    use super::*;

    #[test]
    fn la_tarea_devuelve_su_resultado_una_sola_vez() {
        let contexto = egui::Context::default();
        let mut tarea = Tarea::lanzar(&contexto, "prueba", || Ok(42));

        let mut valor = None;
        for _ in 0..200 {
            if let Some(resultado) = tarea.resultado() {
                valor = Some(resultado.unwrap());
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        assert_eq!(valor, Some(42));
        assert!(
            tarea.resultado().is_none(),
            "el resultado se entregó dos veces"
        );
        assert!(!tarea.en_curso());
    }

    #[test]
    fn un_hilo_que_muere_sin_responder_no_deja_la_tarea_colgada() {
        let contexto = egui::Context::default();
        let mut tarea: Tarea<u32> = Tarea::lanzar(&contexto, "prueba", || {
            panic!("el trabajo revienta");
        });

        let mut resultado = None;
        for _ in 0..200 {
            if let Some(r) = tarea.resultado() {
                resultado = Some(r);
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        assert!(matches!(resultado, Some(Err(yonder::Error::Cancelada))));
    }
}
