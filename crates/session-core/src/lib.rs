use remote_protocol::{BrowserSessionId, HostId, LunaMaxCapability, UserId};
use std::{collections::HashMap, sync::Arc};
use thiserror::Error;
use tokio::sync::{mpsc, RwLock};

#[derive(Clone)]
pub struct HostRoute {
    pub owner_user_id: UserId,
    pub generation: u64,
    pub display_name: String,
    pub luna_max: Option<LunaMaxCapability>,
    pub sender: mpsc::Sender<String>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct HostSummary {
    pub host_id: HostId,
    pub display_name: String,
    pub generation: u64,
    pub luna_max: Option<LunaMaxCapability>,
}

#[derive(Clone, Default)]
pub struct SessionRegistry {
    hosts: Arc<RwLock<HashMap<HostId, HostRoute>>>,
    browsers: Arc<RwLock<HashMap<BrowserSessionId, UserId>>>,
}

#[derive(Debug, Error, PartialEq, Eq)]
pub enum RouteError {
    #[error("host is not connected")]
    HostUnavailable,
    #[error("the authenticated user does not own this host")]
    Forbidden,
    #[error("the host generation is stale")]
    StaleGeneration,
    #[error("browser session is not registered")]
    UnknownBrowserSession,
}

impl SessionRegistry {
    pub async fn register_browser(&self, session_id: BrowserSessionId, user_id: UserId) {
        self.browsers.write().await.insert(session_id, user_id);
    }

    pub async fn unregister_browser(&self, session_id: &BrowserSessionId) {
        self.browsers.write().await.remove(session_id);
    }

    pub async fn register_host(
        &self,
        host_id: HostId,
        owner_user_id: UserId,
        generation: u64,
        display_name: String,
        sender: mpsc::Sender<String>,
    ) -> Result<(), RouteError> {
        let mut hosts = self.hosts.write().await;
        if let Some(current) = hosts.get(&host_id) {
            if current.owner_user_id != owner_user_id {
                return Err(RouteError::Forbidden);
            }
            if generation <= current.generation {
                return Err(RouteError::StaleGeneration);
            }
        }
        hosts.insert(
            host_id,
            HostRoute {
                owner_user_id,
                generation,
                display_name,
                luna_max: None,
                sender,
            },
        );
        Ok(())
    }

    pub async fn update_host_capabilities(
        &self,
        host_id: &HostId,
        generation: u64,
        luna_max: LunaMaxCapability,
    ) -> Result<(), RouteError> {
        let mut hosts = self.hosts.write().await;
        let route = hosts.get_mut(host_id).ok_or(RouteError::HostUnavailable)?;
        if route.generation != generation {
            return Err(RouteError::StaleGeneration);
        }
        route.luna_max = Some(luna_max);
        Ok(())
    }

    pub async fn unregister_host(&self, host_id: &HostId, generation: u64) {
        let mut hosts = self.hosts.write().await;
        if hosts
            .get(host_id)
            .is_some_and(|route| route.generation == generation)
        {
            hosts.remove(host_id);
        }
    }

    pub async fn list_hosts_for_user(&self, user_id: &UserId) -> Vec<HostSummary> {
        let hosts = self.hosts.read().await;
        let mut result = hosts
            .iter()
            .filter(|(_, route)| &route.owner_user_id == user_id)
            .map(|(host_id, route)| HostSummary {
                host_id: host_id.clone(),
                display_name: route.display_name.clone(),
                generation: route.generation,
                luna_max: route.luna_max.clone(),
            })
            .collect::<Vec<_>>();
        result.sort_by(|left, right| left.display_name.cmp(&right.display_name));
        result
    }

    pub async fn route_for_browser(
        &self,
        session_id: &BrowserSessionId,
        host_id: &HostId,
        generation: u64,
    ) -> Result<mpsc::Sender<String>, RouteError> {
        let user_id = self
            .browsers
            .read()
            .await
            .get(session_id)
            .cloned()
            .ok_or(RouteError::UnknownBrowserSession)?;
        let hosts = self.hosts.read().await;
        let route = hosts.get(host_id).ok_or(RouteError::HostUnavailable)?;
        if route.owner_user_id != user_id {
            return Err(RouteError::Forbidden);
        }
        if route.generation != generation {
            return Err(RouteError::StaleGeneration);
        }
        Ok(route.sender.clone())
    }

    pub async fn accepts_host_event(&self, host_id: &HostId, generation: u64) -> bool {
        self.hosts
            .read()
            .await
            .get(host_id)
            .is_some_and(|route| route.generation == generation)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use uuid::Uuid;

    fn ids() -> (UserId, UserId, HostId, BrowserSessionId) {
        (
            UserId(Uuid::new_v4()),
            UserId(Uuid::new_v4()),
            HostId(Uuid::new_v4()),
            BrowserSessionId(Uuid::new_v4()),
        )
    }

    #[tokio::test]
    async fn another_user_cannot_route_to_a_host() {
        let registry = SessionRegistry::default();
        let (owner, attacker, host, browser) = ids();
        let (sender, _) = mpsc::channel(1);
        registry
            .register_host(host.clone(), owner, 1, "owner host".into(), sender)
            .await
            .unwrap();
        registry.register_browser(browser.clone(), attacker).await;
        assert!(matches!(
            registry.route_for_browser(&browser, &host, 1).await,
            Err(RouteError::Forbidden)
        ));
    }

    #[tokio::test]
    async fn stale_host_generation_is_rejected() {
        let registry = SessionRegistry::default();
        let (owner, _, host, browser) = ids();
        let (sender, _) = mpsc::channel(1);
        registry
            .register_host(host.clone(), owner.clone(), 2, "owner host".into(), sender)
            .await
            .unwrap();
        registry.register_browser(browser.clone(), owner).await;
        assert!(matches!(
            registry.route_for_browser(&browser, &host, 1).await,
            Err(RouteError::StaleGeneration)
        ));
        assert!(!registry.accepts_host_event(&host, 1).await);
        assert!(registry.accepts_host_event(&host, 2).await);
    }

    #[tokio::test]
    async fn host_listing_is_scoped_to_the_owner() {
        let registry = SessionRegistry::default();
        let (owner, other, host, _) = ids();
        let (sender, _) = mpsc::channel(1);
        registry
            .register_host(host.clone(), owner.clone(), 1, "desktop".into(), sender)
            .await
            .unwrap();
        assert_eq!(registry.list_hosts_for_user(&owner).await.len(), 1);
        registry
            .update_host_capabilities(
                &host,
                1,
                LunaMaxCapability::Available {
                    model: "gpt-5.6-luna".into(),
                    effort: "max".into(),
                },
            )
            .await
            .unwrap();
        assert!(matches!(
            registry.list_hosts_for_user(&owner).await[0].luna_max,
            Some(LunaMaxCapability::Available { .. })
        ));
        assert!(registry.list_hosts_for_user(&other).await.is_empty());
        registry.unregister_host(&host, 1).await;
        assert!(registry.list_hosts_for_user(&owner).await.is_empty());
    }
}
