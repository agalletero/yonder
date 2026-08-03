//! Por qué la fila de la lista no se hace clicable entera.
//!
//! En egui, dentro de una misma capa, el widget que se registra **después**
//! gana el clic. Una tarjeta que llama a `interact(Sense::click())` sobre su
//! propio marco se registra al final, así que queda por encima de los botones
//! que contiene y se lleva sus clics. El botón nunca los ve: no es que se
//! disparen los dos y haya que desempatar, es que solo se dispara la tarjeta.
//!
//! Costó descubrirlo porque el síntoma no parece un problema de capas: los
//! botones se pintan bien, cambian de color al pasar por encima, y al pulsarlos
//! «no pasa nada». Lo que pasaba era que se abría el panel de detalle.
//!
//! Esta prueba fija ese comportamiento de egui. Si algún día falla, querrá
//! decir que egui ha cambiado el criterio y que la fila podría volver a ser
//! clicable entera sin perder sus botones.

use eframe::egui;

#[test]
fn una_tarjeta_clicable_se_come_el_clic_de_sus_propios_botones() {
    let ctx = egui::Context::default();
    let mut boton_pulsado = false;
    let mut tarjeta_pulsada = false;
    let mut centro = egui::pos2(30.0, 20.0);

    // Tres pasadas: egui necesita colocar antes de poder interactuar.
    for pasada in 0..3 {
        let mut entrada = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(400.0, 200.0),
            )),
            ..Default::default()
        };
        if pasada == 2 {
            entrada.events.push(egui::Event::PointerMoved(centro));
            entrada.events.push(egui::Event::PointerButton {
                pos: centro,
                button: egui::PointerButton::Primary,
                pressed: true,
                modifiers: Default::default(),
            });
            entrada.events.push(egui::Event::PointerButton {
                pos: centro,
                button: egui::PointerButton::Primary,
                pressed: false,
                modifiers: Default::default(),
            });
        }

        boton_pulsado = false;
        tarjeta_pulsada = false;

        let _ = ctx.run(entrada, |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                let interior = egui::Frame::new().show(ui, |ui| {
                    let respuesta = ui.button("Levantar");
                    centro = respuesta.rect.center();
                    if respuesta.clicked() {
                        boton_pulsado = true;
                    }
                });
                if interior.response.interact(egui::Sense::click()).clicked() {
                    tarjeta_pulsada = true;
                }
            });
        });
    }

    assert!(
        tarjeta_pulsada,
        "el clic tenía que llegar a alguna parte; revisar las coordenadas"
    );
    assert!(
        !boton_pulsado,
        "egui ha cambiado: ahora el botón sí ve el clic bajo una tarjeta clicable, \
         así que la fila podría volver a serlo entera"
    );
}
