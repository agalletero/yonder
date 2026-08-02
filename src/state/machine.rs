//! Máquina de estados por túnel (§4).
//!
//! ```text
//! Definido ──▶ Conectando ──▶ Activo ──▶ Cerrando ──▶ Definido
//!                   │            │
//!                   │            ▼
//!                   │        Degradado
//!                   │            │
//!                   ▼            ▼
//!               Reintentando ◀───┘
//!                   │
//!                   ▼
//!                Fallido
//! ```
//!
//! `Degradado` es el estado crítico: maestro vivo pero reenvío caído. Es la
//! diferencia entre una herramienta fiable y una que enseña un punto verde
//! mientras el túnel está muerto. Se detecta comprobando que el puerto local
//! sigue en escucha, no solo que el maestro responde a `-O check`.

use std::fmt;
use std::time::{Duration, Instant};

/// Estado de un túnel.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Estado {
    /// Existe en configuración, sin conexión activa.
    Definido,
    /// Maestro lanzándose o autenticando.
    Conectando,
    /// Maestro vivo y reenvío confirmado.
    Activo,
    /// Maestro vivo pero el reenvío ha caído.
    Degradado,
    /// Bajando el reenvío a petición del usuario.
    Cerrando,
    /// Esperando el siguiente intento con retardo exponencial.
    Reintentando,
    /// Agotados los reintentos o error no recuperable.
    Fallido,
}

impl Estado {
    /// Texto para la interfaz.
    pub fn etiqueta(&self) -> &'static str {
        match self {
            Estado::Definido => "Definido",
            Estado::Conectando => "Conectando",
            Estado::Activo => "Activo",
            Estado::Degradado => "Degradado",
            Estado::Cerrando => "Cerrando",
            Estado::Reintentando => "Reintentando",
            Estado::Fallido => "Fallido",
        }
    }

    /// Explicación de una línea, para la ayuda contextual.
    pub fn explicacion(&self) -> &'static str {
        match self {
            Estado::Definido => "Existe en configuración, sin conexión activa",
            Estado::Conectando => "Maestro lanzándose o autenticando",
            Estado::Activo => "Maestro vivo y reenvío confirmado",
            Estado::Degradado => "El maestro sigue vivo pero el reenvío ha caído",
            Estado::Cerrando => "Bajando el reenvío",
            Estado::Reintentando => "Esperando el siguiente intento",
            Estado::Fallido => "Agotados los reintentos o error no recuperable",
        }
    }

    /// `true` si el estado está en movimiento y merece animación en la interfaz.
    pub fn en_transito(&self) -> bool {
        matches!(
            self,
            Estado::Conectando | Estado::Cerrando | Estado::Reintentando
        )
    }

    /// `true` si el usuario espera que el túnel esté en pie.
    pub fn deberia_estar_arriba(&self) -> bool {
        matches!(
            self,
            Estado::Conectando | Estado::Activo | Estado::Degradado | Estado::Reintentando
        )
    }

    /// `true` si algo va mal y hay que mirarlo.
    pub fn problematico(&self) -> bool {
        matches!(self, Estado::Degradado | Estado::Fallido)
    }

    /// Transiciones admitidas por el diagrama de §4.
    ///
    /// No es decorativo: tener la tabla explícita impide que un camino nuevo se
    /// cuele sin pensarlo, que es como una máquina de estados acaba
    /// convirtiéndose en un montón de banderas.
    pub fn admite(self, siguiente: Estado) -> bool {
        use Estado::*;
        if self == siguiente {
            return true;
        }
        match (self, siguiente) {
            (Definido, Conectando) => true,
            // `Conectando → Degradado` es un túnel que nace zombi: `ssh` acepta
            // el reenvío, el puerto abre y la sonda de salud dice que no
            // transporta. No estaba en el diagrama original de §4 porque el
            // diagrama daba por hecho que abrir el reenvío equivale a que
            // funcione, y resulta que no.
            (Conectando, Activo | Degradado | Reintentando | Fallido) => true,
            (Activo, Degradado | Cerrando | Reintentando) => true,
            (Degradado, Activo | Reintentando | Cerrando) => true,
            (Cerrando, Definido) => true,
            (Reintentando, Conectando | Fallido | Cerrando) => true,
            // Desde Fallido solo se sale reintentando a mano o desactivando.
            (Fallido, Conectando | Definido) => true,
            _ => false,
        }
    }
}

impl fmt::Display for Estado {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.etiqueta())
    }
}

/// Estado completo de un túnel, con lo que la interfaz necesita para pintarlo.
#[derive(Debug, Clone)]
pub struct EstadoTunel {
    pub id: String,
    pub estado: Estado,
    /// Momento del último cambio de estado.
    pub desde: Instant,
    /// Número de reintento en curso; 0 si no se está reintentando.
    pub intento: u32,
    /// Cuándo toca el siguiente intento.
    pub proximo_intento: Option<Instant>,
    /// Último error, tal como se le muestra al usuario.
    pub ultimo_error: Option<String>,
    /// PID del maestro, si está vivo.
    pub pid_maestro: Option<u32>,
}

impl EstadoTunel {
    pub fn nuevo(id: impl Into<String>) -> EstadoTunel {
        EstadoTunel {
            id: id.into(),
            estado: Estado::Definido,
            desde: Instant::now(),
            intento: 0,
            proximo_intento: None,
            ultimo_error: None,
            pid_maestro: None,
        }
    }

    /// Cambia de estado registrando el instante.
    ///
    /// Una transición no prevista se registra como aviso y **se aplica igual**:
    /// negarse dejaría la interfaz mostrando algo que ya no es cierto, que es
    /// justo lo que hay que evitar.
    pub fn transitar(&mut self, siguiente: Estado) {
        if self.estado == siguiente {
            return;
        }
        if !self.estado.admite(siguiente) {
            tracing::warn!(
                id = %self.id,
                desde = %self.estado,
                hacia = %siguiente,
                "transición no prevista en el diagrama de §4"
            );
        }
        tracing::debug!(id = %self.id, desde = %self.estado, hacia = %siguiente, "cambio de estado");
        self.estado = siguiente;
        self.desde = Instant::now();
        if !matches!(siguiente, Estado::Reintentando) {
            self.proximo_intento = None;
        }
        if matches!(siguiente, Estado::Activo) {
            self.intento = 0;
            self.ultimo_error = None;
        }
    }

    /// Fija el estado a partir de lo observado, sin comprobar el diagrama.
    ///
    /// Observar no es transitar. Al abrir la ventana todos los túneles parten de
    /// `Definido` porque el estado en memoria no persiste, y lo que se encuentre
    /// puede ser cualquier cosa: un maestro vivo con su reenvío en pie
    /// (`Activo`), un maestro vivo con el reenvío caído (`Degradado`) o ningún
    /// maestro donde debería haberlo (`Reintentando`). Ninguno de esos saltos
    /// está en el diagrama de §4 —y no debe estarlo, porque el diagrama describe
    /// lo que hacen las acciones, no lo que se encuentra al llegar— así que
    /// pasarlos por `transitar` solo llenaría el registro de avisos falsos.
    pub fn reconstruir(&mut self, observado: Estado) {
        if self.estado == observado {
            return;
        }
        tracing::debug!(
            id = %self.id,
            desde = %self.estado,
            hacia = %observado,
            "estado reconstruido de la observación"
        );
        self.estado = observado;
        self.desde = Instant::now();
        if !matches!(observado, Estado::Reintentando) {
            self.proximo_intento = None;
        }
        if matches!(observado, Estado::Activo) {
            self.intento = 0;
            self.ultimo_error = None;
        }
    }

    /// Tiempo transcurrido en el estado actual.
    pub fn antiguedad(&self) -> Duration {
        self.desde.elapsed()
    }

    /// Segundos que faltan para el siguiente intento.
    pub fn espera_restante(&self) -> Option<Duration> {
        self.proximo_intento
            .map(|momento| momento.saturating_duration_since(Instant::now()))
    }
}

#[cfg(test)]
mod pruebas {
    use super::*;

    #[test]
    fn el_camino_feliz_esta_permitido() {
        assert!(Estado::Definido.admite(Estado::Conectando));
        assert!(Estado::Conectando.admite(Estado::Activo));
        assert!(Estado::Activo.admite(Estado::Cerrando));
        assert!(Estado::Cerrando.admite(Estado::Definido));
    }

    #[test]
    fn el_camino_de_la_caida_esta_permitido() {
        assert!(Estado::Activo.admite(Estado::Degradado));
        assert!(Estado::Degradado.admite(Estado::Reintentando));
        assert!(Estado::Reintentando.admite(Estado::Conectando));
        assert!(Estado::Reintentando.admite(Estado::Fallido));
        // Recuperación sin pasar por Conectando: el reenvío vuelve solo.
        assert!(Estado::Degradado.admite(Estado::Activo));
    }

    #[test]
    fn los_atajos_imposibles_no_estan_permitidos() {
        assert!(!Estado::Definido.admite(Estado::Activo));
        assert!(!Estado::Definido.admite(Estado::Degradado));
        assert!(!Estado::Activo.admite(Estado::Fallido));
        assert!(!Estado::Fallido.admite(Estado::Activo));
    }

    #[test]
    fn activar_limpia_el_contador_y_el_error() {
        let mut estado = EstadoTunel::nuevo("t");
        estado.intento = 4;
        estado.ultimo_error = Some("algo".into());
        estado.transitar(Estado::Conectando);
        estado.transitar(Estado::Activo);
        assert_eq!(estado.intento, 0);
        assert!(estado.ultimo_error.is_none());
    }

    #[test]
    fn degradado_y_fallido_son_los_problematicos() {
        assert!(Estado::Degradado.problematico());
        assert!(Estado::Fallido.problematico());
        assert!(!Estado::Activo.problematico());
        assert!(!Estado::Definido.problematico());
    }

    #[test]
    fn reconstruir_admite_los_saltos_que_transitar_rechaza() {
        // Al reabrir la ventana, un túnel que se dejó activo aparece como
        // Definido en memoria y lo que se observa puede ser cualquier cosa.
        for observado in [Estado::Activo, Estado::Degradado, Estado::Reintentando] {
            let mut estado = EstadoTunel::nuevo("t");
            assert!(
                !Estado::Definido.admite(observado),
                "este salto no debería estar en el diagrama"
            );
            estado.reconstruir(observado);
            assert_eq!(estado.estado, observado);
        }
    }

    #[test]
    fn reconstruir_a_activo_limpia_el_error_anterior() {
        let mut estado = EstadoTunel::nuevo("t");
        estado.intento = 3;
        estado.ultimo_error = Some("algo".into());
        estado.reconstruir(Estado::Activo);
        assert_eq!(estado.intento, 0);
        assert!(estado.ultimo_error.is_none());
    }

    #[test]
    fn degradado_cuenta_como_que_deberia_estar_arriba() {
        // Si no contara, el supervisor lo daría por bajado y no lo recuperaría.
        assert!(Estado::Degradado.deberia_estar_arriba());
        assert!(!Estado::Definido.deberia_estar_arriba());
        assert!(!Estado::Fallido.deberia_estar_arriba());
    }
}
