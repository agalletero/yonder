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
    /// El túnel está en pie sin que nadie lo haya pedido en esta instalación.
    ///
    /// Los maestros se lanzan con `ssh -f` y no son hijos de la aplicación:
    /// sobreviven a cerrar la ventana, a reiniciarla y a perder la base de
    /// datos. Cuando la intención guardada dice «no lo quiero arriba» y la
    /// realidad dice que hay un maestro vivo con el puerto escuchando, la
    /// realidad manda —el túnel transporta— pero la aplicación no lo adopta:
    /// no lo repara si se cae ni lo levanta al arrancar. Solo lo enseña, para
    /// que se pueda cerrar.
    ///
    /// Sin esto, un residual quedaba invisible: la interfaz decía `Definido`
    /// mientras el tráfico pasaba, y no ofrecía ninguna forma de cerrarlo.
    pub residual: bool,
}

impl EstadoTunel {
    /// Dos palabras que quepan en la fila, extraídas del error completo.
    ///
    /// El error entero es una frase —«el puerto escucha pero no se pudo
    /// conectar: connection refused»— que en una lista de quince túneles ni
    /// cabe ni se lee. Aquí se destila a lo que hay que hacer distinto en cada
    /// caso, que es lo único que decide qué botón pulsar. El texto completo
    /// sigue disponible en el detalle y en el globo de ayuda.
    pub fn motivo_breve(&self) -> Option<&'static str> {
        let error = self.ultimo_error.as_deref()?;
        let bajo = error.to_lowercase();
        Some(if bajo.contains("no está en escucha") {
            "puerto caído"
        } else if bajo.contains("ocupado") || bajo.contains("address already in use") {
            "puerto ocupado"
        } else if bajo.contains("no resuelve") || bajo.contains("no se pudo resolver") {
            "sin resolver"
        } else if bajo.contains("escucha pero") || bajo.contains("http") {
            // El zombi va ANTES que los errores de red, y no es un detalle de
            // orden: su mensaje los contiene. «el puerto escucha pero no se
            // pudo conectar: connection refused» es un zombi, no una ruta
            // rota, y se arregla mirando a dónde apunta el reenvío —no la red.
            "no transporta"
        } else if bajo.contains("connection refused") || bajo.contains("rechaz") {
            "sin ruta"
        } else if bajo.contains("timed out") || bajo.contains("agotado") {
            "sin respuesta"
        } else if bajo.contains("permission denied") || bajo.contains("permiso") {
            "sin permiso"
        } else {
            "no responde"
        })
    }

    pub fn nuevo(id: impl Into<String>) -> EstadoTunel {
        EstadoTunel {
            id: id.into(),
            estado: Estado::Definido,
            desde: Instant::now(),
            intento: 0,
            proximo_intento: None,
            ultimo_error: None,
            pid_maestro: None,
            residual: false,
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

    fn con_error(texto: &str) -> EstadoTunel {
        let mut e = EstadoTunel::nuevo("x");
        e.ultimo_error = Some(texto.to_string());
        e
    }

    #[test]
    fn el_motivo_breve_distingue_lo_que_se_arregla_distinto() {
        // Los tres casos que no se resuelven igual: el puerto local caído, el
        // puerto ocupado por otro, y el zombi que abre pero no transporta.
        assert_eq!(
            con_error("el puerto local no está en escucha").motivo_breve(),
            Some("puerto caído")
        );
        assert_eq!(
            con_error("el puerto 3000 está ocupado por «grafana» (PID 42)").motivo_breve(),
            Some("puerto ocupado")
        );
        assert_eq!(
            con_error("el puerto escucha pero no se pudo conectar: connection refused")
                .motivo_breve(),
            Some("no transporta")
        );
    }

    #[test]
    fn sin_error_no_hay_motivo_que_ensenar() {
        assert_eq!(EstadoTunel::nuevo("x").motivo_breve(), None);
    }

    #[test]
    fn un_error_desconocido_no_se_queda_sin_etiqueta() {
        // Vale más un «no responde» honesto que una fila muda.
        assert_eq!(
            con_error("algo raro pasó").motivo_breve(),
            Some("no responde")
        );
    }

    #[test]
    fn el_maestro_sale_de_lo_que_dicen_sus_reenvios() {
        let mut vivo = EstadoTunel::nuevo("a");
        vivo.pid_maestro = Some(123);
        assert_eq!(EstadoMaestro::de_los_tuneles([&vivo]), EstadoMaestro::Vivo);

        let mut reintentando = EstadoTunel::nuevo("b");
        reintentando.estado = Estado::Reintentando;
        assert_eq!(
            EstadoMaestro::de_los_tuneles([&reintentando]),
            EstadoMaestro::Caido
        );

        // Un maestro vivo manda sobre un reenvío caído: es la capa de arriba.
        assert_eq!(
            EstadoMaestro::de_los_tuneles([&vivo, &reintentando]),
            EstadoMaestro::Vivo
        );

        assert_eq!(
            EstadoMaestro::de_los_tuneles([&EstadoTunel::nuevo("c")]),
            EstadoMaestro::Ausente
        );
    }

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

/// Estado de la **conexión maestra** de un host.
///
/// Deliberadamente distinto de `Estado`, que describe un reenvío. Son dos capas
/// y confundirlas fue la causa de que un maestro vivo pudiera quedar invisible:
/// el maestro es por host, los reenvíos cuelgan de él, y «el maestro vive pero
/// este reenvío ha caído» es justo la información que distingue una herramienta
/// fiable de una que enseña puntos verdes sobre algo muerto.
///
/// Solo cuatro valores. Un maestro no se degrada ni se reintenta: eso les pasa
/// a los reenvíos.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum EstadoMaestro {
    /// No hay conexión abierta, ni se ha pedido.
    #[default]
    Ausente,
    /// Se está abriendo o autenticando.
    Levantando,
    /// Abierto y respondiendo.
    Vivo,
    /// Se le pidió estar arriba y no lo está.
    Caido,
}

impl EstadoMaestro {
    /// Deduce el estado del maestro a partir de los reenvíos que cuelgan de él.
    ///
    /// No se consulta el socket aparte: los reenvíos ya llevan el PID del
    /// maestro que los sostiene, puesto por `observar()`. Preguntar dos veces lo
    /// mismo abriría la puerta a que las dos respuestas discrepasen.
    pub fn de_los_tuneles<'a>(estados: impl IntoIterator<Item = &'a EstadoTunel>) -> EstadoMaestro {
        let mut hay_pid = false;
        let mut hay_transito = false;
        let mut hay_esperando = false;

        for estado in estados {
            if estado.pid_maestro.is_some() {
                hay_pid = true;
            }
            match estado.estado {
                Estado::Conectando => hay_transito = true,
                Estado::Reintentando | Estado::Fallido | Estado::Degradado => hay_esperando = true,
                _ => {}
            }
        }

        if hay_pid {
            EstadoMaestro::Vivo
        } else if hay_transito {
            EstadoMaestro::Levantando
        } else if hay_esperando {
            EstadoMaestro::Caido
        } else {
            EstadoMaestro::Ausente
        }
    }

    pub fn etiqueta(&self) -> &'static str {
        match self {
            EstadoMaestro::Ausente => "sin conexión",
            EstadoMaestro::Levantando => "conectando",
            EstadoMaestro::Vivo => "conectado",
            EstadoMaestro::Caido => "conexión caída",
        }
    }
}
