use npc_system::extensions::enabled_extension_ids;
use npc_system::{Simulation, SimulationConfig};

#[test]
fn extension_registry_matches_the_compiled_features() {
    #[cfg(feature = "economy-extension")]
    assert_eq!(enabled_extension_ids(), &["economy"]);

    #[cfg(not(feature = "economy-extension"))]
    assert!(enabled_extension_ids().is_empty());
}

#[test]
fn economy_tick_only_runs_when_the_extension_is_enabled() {
    let mut simulation = Simulation::new(2, 100, 123, SimulationConfig::default()).unwrap();
    let year = simulation.run_year().unwrap();

    #[cfg(feature = "economy-extension")]
    {
        assert!(year.gross_product_cents > 0);
        assert!(year.economic_transactions > 0);
        assert_eq!(year.town_economies.len(), 2);
    }

    #[cfg(not(feature = "economy-extension"))]
    {
        assert_eq!(year.gross_product_cents, 0);
        assert_eq!(year.economic_transactions, 0);
        assert!(year.town_economies.is_empty());
    }
}
