//! `yonder` — arranque. Decide entre modo CLI y modo gráfico.
//!
//! Convención de §11: `anyhow` en el binario, `thiserror` en la biblioteca.

use std::process::ExitCode;

use clap::Parser;

use yonder::cli::{Argumentos, Orden};
use yonder::registro::{self, consola};
use yonder::state::mensaje_completo;

mod ui;

fn main() -> ExitCode {
    let argumentos = Argumentos::parse();
    registro::iniciar(argumentos.verboso);

    let resultado = match argumentos.orden {
        Some(Orden::Gui) => ui::ejecutar(),
        Some(orden) => {
            yonder::cli::ejecutar(orden).map_err(|e| anyhow::anyhow!("{}", mensaje_completo(&e)))
        }
        // Sin argumentos: la ventana si hay servidor gráfico, y si no, la lista.
        // Ejecutar `yonder` por SSH no debe intentar abrir una ventana.
        None => {
            if hay_entorno_grafico() {
                ui::ejecutar()
            } else {
                consola::info("sin servidor gráfico; se muestra la lista (usa «yonder --help»)");
                yonder::cli::ejecutar(Orden::List)
                    .map_err(|e| anyhow::anyhow!("{}", mensaje_completo(&e)))
            }
        }
    };

    match resultado {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            for (indice, causa) in e.chain().enumerate() {
                if indice == 0 {
                    consola::error(causa.to_string());
                } else {
                    consola::error(format!("  causado por: {causa}"));
                }
            }
            ExitCode::FAILURE
        }
    }
}

fn hay_entorno_grafico() -> bool {
    std::env::var_os("WAYLAND_DISPLAY").is_some() || std::env::var_os("DISPLAY").is_some()
}
