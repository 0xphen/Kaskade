use clap::{Parser, ValueEnum};
use std::sync::Arc;

use engine::pulse::{PulseSignal, SpreadConfig, spread::SpreadEngine, types::PulseType};
use engine::registry::{PulseHandler, PulseRegistry};

#[derive(Debug, Clone, ValueEnum)]
pub enum PulseCli {
    Spread,
    Slippage,
    Trend,
}

#[derive(Debug, Parser)]
#[clap(name = "kaskade", version)]
pub struct Cli {
    /// Which pulses to run (comma-separated)
    #[clap(
        long,
        value_enum,
        value_delimiter = ',',
        default_values_t = [PulseCli::Spread]
    )]
    pub pulses: Vec<PulseCli>,

    /// Spread threshold (bps) for triggering SpreadPulse
    #[clap(long, default_value = "3.0")]
    pub spread_threshold_bps: f64,
}

/// Convert CLI pulse selection → internal PulseType enum
pub(crate) fn cli_to_pulse_type(p: &PulseCli) -> PulseType {
    match p {
        PulseCli::Spread => PulseType::Spread,
        PulseCli::Slippage => PulseType::Slippage,
        PulseCli::Trend => PulseType::Trend,
    }
}

/// Build a PulseRegistry from CLI configuration
pub(crate) fn build_registry_from_cli(cli: &Cli) -> PulseRegistry {
    let mut reg = PulseRegistry::new();

    for p in &cli.pulses {
        match p {
            PulseCli::Spread => {
                let engine = SpreadEngine::new(SpreadConfig {
                    window_ms: 5_000,
                    spread_threshold_bps: cli.spread_threshold_bps,
                });
                reg.register_engine(Box::new(engine));

                let handler: PulseHandler = Arc::new(|sig: &PulseSignal| {
                    println!("🔔 [SPREAD] {:?}", sig);
                });

                reg.register_handler(PulseType::Spread, handler);
            }

            PulseCli::Slippage => {
                // TODO: Add SlippageEngine
            }

            PulseCli::Trend => {
                // TODO: Add TrendEngine
            }
        }
    }

    reg
}
