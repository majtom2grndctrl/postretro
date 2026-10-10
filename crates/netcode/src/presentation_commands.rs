// Player-addressed presentation commands: a player event's presentation sent
// to that player's machine, and the client intake that presents it locally.
// See: context/lib/networking.md §Presentation events vs. replicated state

use postretro_entities::SystemReactionCommand;
use postretro_entities::reactions::system_commands::SystemCommandQueue;
use postretro_foundation::Seat;
use postretro_net::transport::NetServer;
use postretro_net::wire::{
    PresentationCommand, ServerPresentationMessage, ServerPresentationPayload,
};

use crate::seat::SeatTable;

/// The wire mirror of a presentation command. `None` for any other command;
/// a sound's emitter anchor never crosses, since a player event has none.
pub fn presentation_command_to_wire(
    command: &SystemReactionCommand,
) -> Option<PresentationCommand> {
    Some(match command {
        SystemReactionCommand::PlaySound { sound, bus, .. } => PresentationCommand::PlaySound {
            sound: sound.clone(),
            bus: bus.clone(),
        },
        SystemReactionCommand::Rumble {
            strong,
            weak,
            duration_ms,
        } => PresentationCommand::Rumble {
            strong: *strong,
            weak: *weak,
            duration_ms: *duration_ms,
        },
        SystemReactionCommand::FlashScreen { color, duration_ms } => {
            PresentationCommand::FlashScreen {
                color: *color,
                duration_ms: *duration_ms,
            }
        }
        SystemReactionCommand::Vignette {
            color,
            strength,
            duration_ms,
        } => PresentationCommand::Vignette {
            color: *color,
            strength: *strength,
            duration_ms: *duration_ms,
        },
        SystemReactionCommand::ScreenShake {
            amplitude,
            duration_ms,
            frequency,
        } => PresentationCommand::ScreenShake {
            amplitude: *amplitude,
            duration_ms: *duration_ms,
            frequency: *frequency,
        },
        // Listed in full so a new command variant fails to build until it is
        // mapped here or deliberately kept off the wire.
        SystemReactionCommand::PushTree { .. }
        | SystemReactionCommand::LoadLevel { .. }
        | SystemReactionCommand::RestartLevel
        | SystemReactionCommand::ReturnToFrontend
        | SystemReactionCommand::PopTree
        | SystemReactionCommand::SetState { .. }
        | SystemReactionCommand::SetSentiment { .. }
        | SystemReactionCommand::AdjustSentiment { .. }
        | SystemReactionCommand::AddOwnerSlot { .. }
        | SystemReactionCommand::CellWrite { .. }
        | SystemReactionCommand::AppendText { .. }
        | SystemReactionCommand::BackspaceText { .. }
        | SystemReactionCommand::ClearText { .. } => return None,
    })
}

/// The local command a received presentation command becomes: the same one a
/// local reaction enqueues. `None`, with a warning, when a number is not
/// finite.
pub fn presentation_command_from_wire(
    command: PresentationCommand,
) -> Option<SystemReactionCommand> {
    let finite = |values: &[f32]| values.iter().all(|value| value.is_finite());
    let (local, numbers): (SystemReactionCommand, Vec<f32>) = match command {
        PresentationCommand::PlaySound { sound, bus } => (
            SystemReactionCommand::PlaySound {
                sound,
                bus,
                at: None,
            },
            Vec::new(),
        ),
        PresentationCommand::Rumble {
            strong,
            weak,
            duration_ms,
        } => (
            SystemReactionCommand::Rumble {
                strong,
                weak,
                duration_ms,
            },
            [strong, duration_ms].into_iter().chain(weak).collect(),
        ),
        PresentationCommand::FlashScreen { color, duration_ms } => (
            SystemReactionCommand::FlashScreen { color, duration_ms },
            color.into_iter().chain([duration_ms]).collect(),
        ),
        PresentationCommand::Vignette {
            color,
            strength,
            duration_ms,
        } => (
            SystemReactionCommand::Vignette {
                color,
                strength,
                duration_ms,
            },
            [strength, duration_ms]
                .into_iter()
                .chain(color.into_iter().flatten())
                .collect(),
        ),
        PresentationCommand::ScreenShake {
            amplitude,
            duration_ms,
            frequency,
        } => (
            SystemReactionCommand::ScreenShake {
                amplitude,
                duration_ms,
                frequency,
            },
            [amplitude, duration_ms]
                .into_iter()
                .chain(frequency)
                .collect(),
        ),
    };
    if !finite(&numbers) {
        log::warn!(
            "[Netcode] dropped a forwarded presentation command carrying a non-finite number"
        );
        return None;
    }
    Some(local)
}

/// Present every command player events routed this frame. A seat bound to a
/// remote client receives one unreliable packet; any other seat — the host's
/// own player (`Seat(0)`) and single player — presents locally through `queue`.
/// A remote seat with no client bound (held, disconnected) presents nothing.
/// A failed send is a dropped cosmetic, never retained work.
pub fn route_player_presentation(
    queue: &SystemCommandQueue,
    server: Option<&mut NetServer>,
    seats: Option<&SeatTable>,
) {
    let routed = queue.take_routed();
    if routed.is_empty() {
        return;
    }
    let mut server = server;
    for (seat, command) in routed {
        let client = seats.and_then(|seats| seats.client_for_seat(seat));
        if let (Some(client_id), Some(server)) = (client, server.as_deref_mut()) {
            let Some(command) = presentation_command_to_wire(&command) else {
                continue;
            };
            let _ = server.send_presentation(
                client_id,
                ServerPresentationMessage {
                    payload: ServerPresentationPayload::Command(command),
                },
            );
        } else if seats.is_none() || server.is_none() || seat == Seat(0) {
            // Single player / no net, or the host's own seat: present here.
            // The fire context's `presentation_seat` is already restored to
            // `None` by the time the app drain routes, so this push lands in
            // the local queue instead of being re-held as routed.
            queue.push(command);
        } else {
            // A remote seat with no client bound (held, disconnected): the
            // cosmetic belongs to that player's machine alone, so it must not
            // fall back to the host's screen.
            log::debug!(
                "[Netcode] dropped a presentation command for seat {} with no bound client",
                seat.0
            );
        }
    }
}

/// Split the commands out of a frame's received presentation; the rest go to
/// the passive intake.
pub fn split_presentation_commands(
    messages: Vec<ServerPresentationMessage>,
) -> (Vec<PresentationCommand>, Vec<ServerPresentationMessage>) {
    let mut commands = Vec::new();
    let mut passive = Vec::with_capacity(messages.len());
    for message in messages {
        match message.payload {
            ServerPresentationPayload::Command(command) => commands.push(command),
            payload => passive.push(ServerPresentationMessage { payload }),
        }
    }
    (commands, passive)
}

/// Enqueue received presentation commands as local commands, so this
/// machine's accommodations (flash limiter, reduce motion) apply to them as to
/// its own.
pub fn ingest_presentation_commands(
    commands: Vec<PresentationCommand>,
    queue: &SystemCommandQueue,
) {
    for command in commands {
        if let Some(local) = presentation_command_from_wire(command) {
            queue.push(local);
        }
    }
}

#[cfg(test)]
mod tests;
