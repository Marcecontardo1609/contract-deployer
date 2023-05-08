use std::collections::HashMap;
use std::sync::Arc;

use ethers::prelude::encode_function_data;
use ethers::types::Address;
use eyre::{Context as _, ContextCompat};
use serde::{Deserialize, Serialize};
use tracing::instrument;

use crate::common_keys::RpcSigner;
use crate::ethers_utils::TransactionBuilder;
use crate::forge_utils::{
    ContractSpec, ForgeCreate, ForgeInspectAbi, ForgeOutput,
};
use crate::identity_manager::WorldIDIdentityManagersDeployment;
use crate::types::GroupId;
use crate::{Config, DeploymentContext};

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct WorldIdRouterDeployment {
    pub impl_v1_deployment: ForgeOutput,
    pub proxy_deployment: ForgeOutput,
    pub entries: HashMap<GroupId, Address>,
}

#[instrument(skip_all)]
async fn deploy_world_id_router_v1(
    context: &DeploymentContext,
    first_group_address: Address,
) -> eyre::Result<WorldIdRouterDeployment> {
    if let Some(previous_deployment) = context.report.world_id_router.as_ref() {
        return Ok(previous_deployment.clone());
    }

    let contract_spec = ContractSpec::name("WorldIDRouter");
    let impl_spec = ContractSpec::name("WorldIDRouterImplV1");

    let impl_v1_deployment = ForgeCreate::new(impl_spec.clone())
        .with_cwd("./world-id-contracts")
        .with_private_key(context.args.private_key.to_string())
        .with_rpc_url(context.args.rpc_url.to_string())
        .with_override_nonce(context.next_nonce())
        .run()
        .await?;

    let impl_abi = ForgeInspectAbi::new(impl_spec.clone())
        .with_cwd("./world-id-contracts")
        .run()
        .await?;

    let initialize_func = impl_abi.function("initialize")?;

    let call_data = encode_function_data(initialize_func, first_group_address)?;

    let proxy_deployment = ForgeCreate::new(contract_spec)
        .with_cwd("./world-id-contracts")
        .with_private_key(context.args.private_key.to_string())
        .with_rpc_url(context.args.rpc_url.to_string())
        .with_override_nonce(context.next_nonce())
        .with_constructor_arg(format!("{:?}", impl_v1_deployment.deployed_to))
        .with_constructor_arg(call_data)
        .run()
        .await?;

    Ok(WorldIdRouterDeployment {
        impl_v1_deployment,
        proxy_deployment,
        entries: maplit::hashmap! {
            GroupId(0) => first_group_address
        },
    })
}

#[instrument(skip(context))]
async fn update_group_route(
    context: &DeploymentContext,
    world_id_router_address: Address,
    group_id: GroupId,
    new_target_address: Address,
) -> eyre::Result<()> {
    let impl_spec = ContractSpec::name("WorldIDRouterImplV1");

    let impl_abi = ForgeInspectAbi::new(impl_spec.clone())
        .with_cwd("./world-id-contracts")
        .run()
        .await?;

    let signer = context.dep_map.get::<RpcSigner>().await;

    let tx = TransactionBuilder::default()
        .signer(signer.clone())
        .abi(impl_abi.clone())
        .function_name("updateGroup")
        .args((group_id.0 as u64, new_target_address))
        .to(world_id_router_address)
        .context(context)
        .build()?;

    tx.send().await?;

    Ok(())
}

#[instrument(skip(context))]
async fn add_group_route(
    context: &DeploymentContext,
    world_id_router_address: Address,
    group_id: GroupId,
    new_target_address: Address,
) -> eyre::Result<()> {
    let impl_spec = ContractSpec::name("WorldIDRouterImplV1");

    let impl_abi = ForgeInspectAbi::new(impl_spec.clone())
        .with_cwd("./world-id-contracts")
        .run()
        .await?;

    let signer = context.dep_map.get::<RpcSigner>().await;

    let tx = TransactionBuilder::default()
        .signer(signer.clone())
        .abi(impl_abi.clone())
        .function_name("addGroup")
        .args((group_id.0 as u64, new_target_address))
        .to(world_id_router_address)
        .context(context)
        .build()?;

    tx.send().await?;

    Ok(())
}

#[instrument(name = "world_id_router", skip_all)]
pub async fn deploy(
    context: Arc<DeploymentContext>,
    config: Arc<Config>,
    identity_managers: &WorldIDIdentityManagersDeployment,
) -> eyre::Result<WorldIdRouterDeployment> {
    let first_group = identity_managers
        .groups
        .get(&GroupId(0))
        .context("Missing group 0")?;

    let mut world_id_router_deployment = deploy_world_id_router_v1(
        context.as_ref(),
        first_group.proxy_deployment.deployed_to,
    )
    .await
    .context("deploying world id router implementation")?;

    let mut group_ids: Vec<_> = config.groups.keys().copied().collect();
    group_ids.sort();

    // TODO: Add removal option
    for group_id in group_ids {
        let group_identity_manager_address = identity_managers
            .groups
            .get(&group_id)
            .context("Missing group")?
            .proxy_deployment
            .deployed_to;

        if let Some(current_group_address) =
            world_id_router_deployment.entries.get_mut(&group_id)
        {
            if *current_group_address != group_identity_manager_address {
                update_group_route(
                    context.as_ref(),
                    world_id_router_deployment.proxy_deployment.deployed_to,
                    group_id,
                    group_identity_manager_address,
                )
                .await?;

                *current_group_address = group_identity_manager_address;
            }
        } else {
            add_group_route(
                context.as_ref(),
                world_id_router_deployment.proxy_deployment.deployed_to,
                group_id,
                group_identity_manager_address,
            )
            .await?;

            world_id_router_deployment
                .entries
                .insert(group_id, group_identity_manager_address);
        }
    }

    Ok(world_id_router_deployment)
}
