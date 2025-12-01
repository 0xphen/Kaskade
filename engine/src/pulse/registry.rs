use std::collections::HashMap;
use std::sync::Arc;

use super::{PulseEngine, PulseSignal, PulseType};
use crate::normalized_quote::NormalizedQuote;

/// A PulseHandler is a thread-safe callback that receives a reference to a PulseSignal.
pub type PulseHandler = Arc<dyn Fn(&PulseSignal) + Send + Sync + 'static>;

#[derive(Default)]
pub struct PulseRegistry {
    /// All registered engines, keyed by pulse type.
    engines: HashMap<PulseType, Box<dyn PulseEngine>>,
    /// One or more handlers per pulse type.
    handlers: HashMap<PulseType, Vec<PulseHandler>>,
    /// Optional callback invoked when a handler panics.
    on_handler_panic: Option<Arc<dyn Fn(PulseType, &PulseSignal) + Send + Sync + 'static>>,
}

impl PulseRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    /// Attach a callback to be invoked whenever a handler panics.
    pub fn with_panic_hook<F>(mut self, f: F) -> Self
    where
        F: Fn(PulseType, &PulseSignal) + Send + Sync + 'static,
    {
        self.on_handler_panic = Some(Arc::new(f));
        self
    }

    /// Register a new engine. Replaces any existing engine for that PulseType.
    pub fn register_engine(&mut self, engine: Box<dyn PulseEngine>) {
        let pulse_type = engine.kind();
        self.engines.insert(pulse_type, engine);
    }

    /// Register a handler (callback) for a given pulse type.
    pub fn register_handler(&mut self, pulse_type: PulseType, handler: PulseHandler) {
        self.handlers.entry(pulse_type).or_default().push(handler);
    }

    /// Convenience: check if an engine for a pulse type exists.
    pub fn has_engine(&self, pulse_type: PulseType) -> bool {
        self.engines.contains_key(&pulse_type)
    }

    /// Feed a quote to *all* engines. If any engine emits a pulse,
    /// notify all handlers registered for that pulse type.
    ///
    /// Design:
    /// - Engines are trusted: if an engine panics, we let it propagate.
    /// - Handlers are untrusted: each handler run is wrapped in catch_unwind,
    ///   so one bad handler does not prevent others from running.
    pub fn process_quote(&mut self, q: NormalizedQuote) {
        for (pulse_type, engine) in self.engines.iter_mut() {
            let maybe_signal = engine.on_quote(q.clone());

            if let Some(signal) = maybe_signal {
                if let Some(handlers) = self.handlers.get(pulse_type) {
                    for handler in handlers {
                        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                            handler(&signal);
                        }));

                        if result.is_err() {
                            if let Some(ref hook) = self.on_handler_panic {
                                hook(pulse_type.clone(), &signal);
                            }
                        }
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex};

    use super::*;
    use crate::normalized_quote::NormalizedQuote;
    use corelib::models::QuoteSide;

    // Mock Engine
    #[derive(Clone)]
    struct MockEngine {
        pulse_type: PulseType,
        emitted_signal: Option<PulseSignal>,
        received: Arc<Mutex<Vec<NormalizedQuote>>>,
    }

    impl MockEngine {
        fn new(pulse_type: PulseType, emitted_signal: Option<PulseSignal>) -> Self {
            Self {
                pulse_type,
                emitted_signal,
                received: Arc::new(Mutex::new(vec![])),
            }
        }
    }

    impl PulseEngine for MockEngine {
        fn kind(&self) -> PulseType {
            self.pulse_type.clone()
        }

        fn on_quote(&mut self, q: NormalizedQuote) -> Option<PulseSignal> {
            self.received.lock().unwrap().push(q);
            self.emitted_signal.clone()
        }
    }

    // Dummy Quote
    fn q(price: u64) -> NormalizedQuote {
        NormalizedQuote {
            ts_ms: 12345,
            price: price as f64,
            amount_in: 100.0,
            amount_out: price as f64 * 2.0,
            side: QuoteSide::Bid,
        }
    }

    #[test]
    fn registers_engine_and_overwrites_previous() {
        let mut reg = PulseRegistry::default();

        reg.register_engine(Box::new(MockEngine::new(PulseType::Spread, None)));
        reg.register_engine(Box::new(MockEngine::new(PulseType::Spread, None)));

        assert!(reg.has_engine(PulseType::Spread));
        assert_eq!(reg.engines.len(), 1);
    }

    #[test]
    fn registers_handlers_and_fires_them() {
        let mut reg = PulseRegistry::default();

        let signal = PulseSignal::spread(1.0, 1.1, 5.0, 10.0, 999);

        reg.register_engine(Box::new(MockEngine::new(
            PulseType::Spread,
            Some(signal.clone()),
        )));

        let seen = Arc::new(Mutex::new(vec![]));
        let seen_clone = seen.clone();

        reg.register_handler(
            PulseType::Spread,
            Arc::new(move |sig: &PulseSignal| {
                seen_clone.lock().unwrap().push(sig.pulse_type.clone());
            }),
        );

        reg.process_quote(q(100));

        assert_eq!(*seen.lock().unwrap(), vec![PulseType::Spread]);
    }

    #[test]
    fn multiple_handlers_receive_same_signal() {
        let mut reg = PulseRegistry::default();

        let signal = PulseSignal::spread(1.0, 1.2, 20.0, 30.0, 555);
        reg.register_engine(Box::new(MockEngine::new(PulseType::Spread, Some(signal))));

        let c1 = Arc::new(Mutex::new(0));
        let c2 = Arc::new(Mutex::new(0));

        let c1_clone = c1.clone();
        let c2_clone = c2.clone();

        reg.register_handler(
            PulseType::Spread,
            Arc::new(move |_| {
                *c1_clone.lock().unwrap() += 1;
            }),
        );

        reg.register_handler(
            PulseType::Spread,
            Arc::new(move |_| {
                *c2_clone.lock().unwrap() += 1;
            }),
        );

        reg.process_quote(q(42));

        assert_eq!(*c1.lock().unwrap(), 1);
        assert_eq!(*c2.lock().unwrap(), 1);
    }

    #[test]
    fn engine_does_not_trigger_handlers_if_no_signal() {
        let mut reg = PulseRegistry::default();

        reg.register_engine(Box::new(MockEngine::new(PulseType::Spread, None)));

        let hit = Arc::new(Mutex::new(false));
        let hitc = hit.clone();

        reg.register_handler(
            PulseType::Spread,
            Arc::new(move |_| {
                *hitc.lock().unwrap() = true;
            }),
        );

        reg.process_quote(q(123));

        assert!(!*hit.lock().unwrap());
    }

    #[test]
    fn all_engines_are_invoked() {
        let mut reg = PulseRegistry::default();

        let e1 = MockEngine::new(PulseType::Spread, None);
        let e2 = MockEngine::new(PulseType::Slippage, None);

        let r1 = e1.received.clone();
        let r2 = e2.received.clone();

        reg.register_engine(Box::new(e1));
        reg.register_engine(Box::new(e2));

        reg.process_quote(q(77));

        assert_eq!(r1.lock().unwrap().len(), 1);
        assert_eq!(r2.lock().unwrap().len(), 1);
    }

    #[test]
    fn quote_is_cloned_for_each_engine() {
        let mut reg = PulseRegistry::default();

        let e1 = MockEngine::new(PulseType::Spread, None);
        let e2 = MockEngine::new(PulseType::Trend, None);

        let r1 = e1.received.clone();
        let r2 = e2.received.clone();

        reg.register_engine(Box::new(e1));
        reg.register_engine(Box::new(e2));

        reg.process_quote(q(500));

        let q1 = &r1.lock().unwrap()[0];
        let q2 = &r2.lock().unwrap()[0];

        assert_eq!(q1.price, 500.0);
        assert_eq!(q2.price, 500.0);
        assert!(!std::ptr::eq(q1, q2));
    }

    #[test]
    fn handler_panics_do_not_stop_other_handlers() {
        let mut reg = PulseRegistry::default();

        let signal = PulseSignal::slippage(0.02, 0.05, 888);

        reg.register_engine(Box::new(MockEngine::new(PulseType::Slippage, Some(signal))));

        let hits = Arc::new(Mutex::new(0));
        let hits_clone = hits.clone();

        // First handler panics
        reg.register_handler(PulseType::Slippage, Arc::new(|_| panic!("boom")));

        // Second handler must still run
        reg.register_handler(
            PulseType::Slippage,
            Arc::new(move |_| {
                *hits_clone.lock().unwrap() += 1;
            }),
        );

        // Registry must isolate handler panics internally
        reg.process_quote(q(9));

        assert_eq!(*hits.lock().unwrap(), 1);
    }
}
