// Copyright 2018 Kodebox, Inc.
// This file is part of CodeChain.
//
// This program is free software: you can redistribute it and/or modify
// it under the terms of the GNU Affero General Public License as
// published by the Free Software Foundation, either version 3 of the
// License, or (at your option) any later version.
//
// This program is distributed in the hope that it will be useful,
// but WITHOUT ANY WARRANTY; without even the implied warranty of
// MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE.  See the
// GNU Affero General Public License for more details.
//
// You should have received a copy of the GNU Affero General Public License
// along with this program.  If not, see <https://www.gnu.org/licenses/>.

#[macro_use]
extern crate clap;
extern crate futures;

#[macro_use]
extern crate log;
extern crate tokio_core;

#[macro_use]
extern crate serde_derive;

extern crate app_dirs;
extern crate codechain_core as ccore;
extern crate codechain_discovery as cdiscovery;
extern crate codechain_key as ckey;
extern crate codechain_keystore as ckeystore;
#[macro_use]
extern crate codechain_logger as clogger;
extern crate codechain_network as cnetwork;
extern crate codechain_reactor as creactor;
extern crate codechain_rpc as crpc;
extern crate codechain_sync as csync;
extern crate codechain_types as ctypes;
extern crate ctrlc;
extern crate env_logger;
extern crate fdlimit;
extern crate panic_hook;
extern crate parking_lot;
extern crate rpassword;
#[cfg(feature = "stratum")]
extern crate stratum;
extern crate toml;

mod account_command;
mod config;
mod rpc;
mod rpc_apis;

use std::fs;
use std::path::Path;
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use app_dirs::AppInfo;
use ccore::{AccountProvider, ClientService, EngineType, Miner, MinerOptions, MinerService, Spec};
use cdiscovery::{KademliaConfig, KademliaExtension, UnstructuredConfig, UnstructuredExtension};
use ckeystore::accounts_dir::RootDiskDirectory;
use ckeystore::KeyStore;
use clap::ArgMatches;
use clogger::LoggerConfig;
use cnetwork::{NetworkConfig, NetworkControl, NetworkService, SocketAddr};
use creactor::EventLoop;
use crpc::{HttpServer, IpcServer};
use csync::{BlockSyncExtension, ParcelSyncExtension, SnapshotService};
use ctrlc::CtrlC;
use fdlimit::raise_fd_limit;
use parking_lot::{Condvar, Mutex};

use self::account_command::run_account_command;
use self::rpc::{HttpConfiguration as RpcHttpConfig, IpcConfiguration as RpcIpcConfig};

pub const APP_INFO: AppInfo = AppInfo {
    name: "codechain",
    author: "Kodebox",
};

pub fn rpc_start<NC>(cfg: RpcHttpConfig, deps: Arc<rpc_apis::ApiDependencies<NC>>) -> Result<HttpServer, String>
where
    NC: NetworkControl + Send + Sync, {
    info!("RPC Listening on {}", cfg.port);
    rpc::new_http(cfg, deps)
}

pub fn rpc_ipc_start<NC>(cfg: RpcIpcConfig, deps: Arc<rpc_apis::ApiDependencies<NC>>) -> Result<IpcServer, String>
where
    NC: NetworkControl + Send + Sync, {
    info!("IPC Listening on {}", cfg.socket_addr);
    rpc::new_ipc(cfg, deps)
}

pub fn network_start(cfg: &NetworkConfig) -> Result<Arc<NetworkService>, String> {
    info!("Handshake Listening on {}", cfg.port);
    let address = SocketAddr::v4(127, 0, 0, 1, cfg.port);
    let service = NetworkService::start(address, cfg.min_peers, cfg.max_peers)
        .map_err(|e| format!("Network service error: {:?}", e))?;

    Ok(service)
}

pub fn discovery_start(service: &NetworkService, cfg: &config::Network) -> Result<(), String> {
    match cfg.discovery_type.as_ref() {
        "unstructured" => {
            let config = UnstructuredConfig {
                bucket_size: cfg.discovery_bucket_size,
                t_refresh: cfg.discovery_refresh,
            };
            let unstructured = UnstructuredExtension::new(config);
            service.set_routing_table(&*unstructured);
            service.register_extension(unstructured)?;
            cinfo!(DISCOVERY, "Node runs with unstructured discovery");
        }
        "kademlia" => {
            let config = KademliaConfig {
                bucket_size: cfg.discovery_bucket_size,
                t_refresh: cfg.discovery_refresh,
            };
            let kademlia = KademliaExtension::new(config);
            service.set_routing_table(&*kademlia);
            service.register_extension(kademlia)?;
            cinfo!(DISCOVERY, "Node runs with kademlia discovery");
        }
        discovery_type => return Err(format!("Unknown discovery {}", discovery_type)),
    }
    Ok(())
}

pub fn client_start(cfg: &config::Config, spec: &Spec, miner: Arc<Miner>) -> Result<ClientService, String> {
    info!("Starting client");
    let client_path = Path::new(&cfg.operating.db_path);
    let client_config = Default::default();
    let service = ClientService::start(client_config, &spec, &client_path, miner)
        .map_err(|e| format!("Client service error: {}", e))?;

    Ok(service)
}

#[cfg(all(unix, target_arch = "x86_64"))]
fn main() -> Result<(), String> {
    panic_hook::set();

    // Always print backtrace on panic.
    ::std::env::set_var("RUST_BACKTRACE", "1");

    run()
}

fn run() -> Result<(), String> {
    let yaml = load_yaml!("codechain.yml");
    let matches = clap::App::from_yaml(yaml).get_matches();

    match matches.subcommand {
        Some(_) => run_subcommand(matches),
        None => run_node(matches),
    }
}

fn run_subcommand(matches: ArgMatches) -> Result<(), String> {
    let subcommand = matches.subcommand.unwrap();
    if subcommand.name == "account" {
        run_account_command(subcommand.matches)
    } else {
        Err("Invalid subcommand".to_string())
    }
}

fn load_config(matches: &ArgMatches) -> Result<config::Config, String> {
    const DEFAULT_CONFIG_PATH: &'static str = "codechain/config/presets/config.dev.toml";

    let config_path = matches.value_of("config").unwrap_or(DEFAULT_CONFIG_PATH);
    let mut config = config::load(&config_path)?;
    config.ipc.overwrite_with(&matches)?;
    config.operating.overwrite_with(&matches)?;
    config.mining.overwrite_with(&matches)?;
    config.network.overwrite_with(&matches)?;
    config.rpc.overwrite_with(&matches)?;
    config.snapshot.overwrite_with(&matches)?;
    config.stratum.overwrite_with(&matches)?;

    Ok(config)
}

fn new_miner(config: &config::Config, spec: &Spec, ap: Arc<AccountProvider>) -> Result<Arc<Miner>, String> {
    let miner_options = MinerOptions {
        mem_pool_size: config.mining.mem_pool_size,
        mem_pool_memory_limit: match config.mining.mem_pool_mem_limit {
            0 => None,
            mem_size => Some(mem_size * 1024 * 1024),
        },
        new_work_notify: config.mining.notify_work.clone(),
        force_sealing: config.mining.force_sealing,
        reseal_min_period: Duration::from_millis(config.mining.reseal_min_period),
        reseal_max_period: Duration::from_millis(config.mining.reseal_max_period),
        work_queue_size: config.mining.work_queue_size,
        ..MinerOptions::default()
    };

    let miner = Miner::new(miner_options, spec, Some(ap.clone()));


    match miner.engine_type() {
        EngineType::PoW => {
            let author = config.mining.author;
            match author {
                Some(author) => miner.set_author(author),
                None => return Err("mining.author is not specified".to_string()),
            }
        }
        EngineType::InternalSealing => match config.mining.engine_signer {
            Some(engine_signer) => match ap.has_account(engine_signer) {
                Ok(has_account) if !has_account => {
                    return Err("mining.engine_signer is not found in AccountProvider".to_string())
                }
                Ok(..) => {
                    match config.mining.password_path {
                        None => return Err("mining.password_path is not specified".to_string()),
                        Some(ref password_path) => match fs::read_to_string(password_path) {
                            Ok(password) => {
                                miner.set_engine_signer(engine_signer, password).map_err(|err| format!("{:?}", err))?
                            }
                            Err(_) => return Err(format!("Failed to read the password file")),
                        },
                    }
                    miner.set_engine_signer(engine_signer, "password".to_string()).map_err(|err| format!("{:?}", err))?;
                }
                Err(e) => {
                    return Err(format!("Error while checking whether engine_signer is in AccountProvider: {:?}", e))
                }
            },
            None => return Err("mining.engine_signer is not specified".to_string()),
        },
        EngineType::Solo => (),
    }

    Ok(miner)
}

fn run_node(matches: ArgMatches) -> Result<(), String> {
    // increase max number of open files
    raise_fd_limit();

    let _event_loop = EventLoop::spawn();
    let config = load_config(&matches)?;
    let spec = config.operating.chain.spec()?;

    let instance_id = config.operating.instance_id.unwrap_or(SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("Current time should be later than unix epoch")
        .subsec_nanos() as usize);
    clogger::init(&LoggerConfig::new(instance_id)).expect("Logger must be successfully initialized");

    // FIXME: Handle IO error.
    let keystore_dir =
        RootDiskDirectory::create(config.operating.keys_path.clone()).expect("Cannot read key path directory");
    let keystore = KeyStore::open(Box::new(keystore_dir)).unwrap();
    let ap = AccountProvider::new(keystore);
    let miner = new_miner(&config, &spec, ap.clone())?;
    let client = client_start(&config, &spec, miner.clone())?;

    let network_service = {
        if !config.network.disable {
            let network_config = (&config.network).into();
            let service = network_start(&network_config)?;

            if config.network.discovery {
                discovery_start(&service, &config.network)?;
            } else {
                cwarn!(DISCOVERY, "Node runs without discovery extension");
            }

            if config.network.sync {
                let sync = BlockSyncExtension::new(client.client());
                service.register_extension(sync.clone())?;
                client.client().add_notify(sync.clone());
            }
            if config.network.parcel_relay {
                service.register_extension(ParcelSyncExtension::new(client.client()))?;
            }
            if let Some(consensus_extension) = spec.engine.network_extension() {
                service.register_extension(consensus_extension)?;
            }

            for address in network_config.bootstrap_addresses {
                service.connect_to(address)?;
            }
            Some(service)
        } else {
            None
        }
    };

    let rpc_apis_deps = Arc::new(rpc_apis::ApiDependencies {
        client: client.client(),
        miner: Arc::clone(&miner),
        network_control: network_service.as_ref().map(Arc::clone),
        account_provider: ap,
    });

    let _rpc_server = {
        if !config.rpc.disable {
            let rpc_config = (&config.rpc).into();
            Some(rpc_start(rpc_config, Arc::clone(&rpc_apis_deps))?)
        } else {
            None
        }
    };

    let _ipc_server = {
        if !config.ipc.disable {
            let ipc_config = (&config.ipc).into();
            Some(rpc_ipc_start(ipc_config, Arc::clone(&rpc_apis_deps))?)
        } else {
            None
        }
    };

    let _snapshot_service = {
        if !config.snapshot.disable {
            // FIXME: Get snapshot period from genesis block
            let service = SnapshotService::new(client.client(), config.snapshot.path, 1 << 14);
            client.client().add_notify(service.clone());
            Some(service)
        } else {
            None
        }
    };

    // drop the spec to free up genesis state.
    drop(spec);

    cinfo!(TEST_SCRIPT, "Initialization complete");

    wait_for_exit();

    Ok(())
}

fn wait_for_exit() {
    let exit = Arc::new((Mutex::new(()), Condvar::new()));

    // Handle possible exits
    let e = exit.clone();
    CtrlC::set_handler(move || {
        e.1.notify_all();
    });

    // Wait for signal
    let mut l = exit.0.lock();
    let _ = exit.1.wait(&mut l);
}
