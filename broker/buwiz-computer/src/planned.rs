//! The slots of the providers that aren't built: Lume, sbx, EC2, boat.dev and Fly. Each
//! answers with its sentence, so a saved computer of that kind shows what's wrong and nothing
//! panics.

use async_trait::async_trait;

use crate::{Machine, Power, Problem, Provider, ProviderKind, Result, Sleep, Spec, Woke};

pub struct Planned(pub ProviderKind);

#[async_trait]
impl Provider for Planned {
    fn kind(&self) -> ProviderKind {
        self.0
    }

    async fn check(&self) -> Result<()> {
        Err(Problem::NotBuilt(self.0))
    }

    async fn create(&self, _: &Spec) -> Result<Machine> {
        Err(Problem::NotBuilt(self.0))
    }

    async fn power(&self, _: &Machine) -> Result<Power> {
        Err(Problem::NotBuilt(self.0))
    }

    async fn wake(&self, _: &Machine) -> Result<Woke> {
        Err(Problem::NotBuilt(self.0))
    }

    async fn sleep(&self, _: &Machine, _: Sleep) -> Result<()> {
        Err(Problem::NotBuilt(self.0))
    }

    async fn destroy(&self, _: &Machine) -> Result<()> {
        Err(Problem::NotBuilt(self.0))
    }
}
