#![forbid(unsafe_code)]

use lorelei_core::error::LoreleiError;
use lorelei_core::traits::LoreStore;
use lorelei_core::types::{Pearl, TenantId};

use crate::qdrant::VectorHit;

pub struct ResolvedHit {
    pub score: f32,
    pub pearl: Pearl,
}

pub async fn resolve_hits<S: LoreStore>(
    store: &S,
    tenant_id: TenantId,
    hits: Vec<VectorHit>,
) -> Result<(Vec<ResolvedHit>, Vec<lorelei_core::types::PearlId>), LoreleiError> {
    let mut resolved = Vec::new();
    let mut ignored = Vec::new();

    for hit in hits {
        match store.get_pearl(tenant_id, hit.pearl_id, false).await? {
            Some(pearl) => resolved.push(ResolvedHit {
                score: hit.score,
                pearl,
            }),
            None => ignored.push(hit.pearl_id),
        }
    }

    Ok((resolved, ignored))
}
