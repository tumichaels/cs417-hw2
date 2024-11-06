use clap::Parser;
use rand::{Rng, SeedableRng};
use rand::rngs::StdRng;
use std::collections::HashMap;
use std::time::Instant;
use tonic::transport::Channel;
use tonic::Request;
use path_oram::{
    path_oram_client::PathOramClient, 
    Block, 
    SetupRequest, SetupResponse, 
    ReadBlockRequest, ReadBlockResponse,
    WriteBlockRequest, WriteBlockResponse, 
    PrintRequest,
};

pub mod path_oram {
    tonic::include_proto!("path_oram");
}

#[derive(Parser, Debug)]
#[command(name = "Path ORAM Client", about = "Path ORAM gRPC Client in Rust")]
struct Args {
    /// Total number of nodes (N)
    #[arg(long)]
    n: u32,

    /// Bucket size (Z)
    #[arg(long)]
    z: i32,

    /// Block size (B)
    #[arg(long)]
    b: i32,
}

macro_rules! debug_rpc_call {
    ($client:expr, $request:expr) => {
        if cfg!(debug_assertions) {
            if let Err(e) = $client.print($request).await {
                println!("Debug RPC call failed: {:?}", e);
            }
        }
    };
}

macro_rules! debug_println {
    ($($arg:tt)*) => (if ::std::cfg!(debug_assertions) { ::std::println!($($arg)*); })
}

pub struct PathORAMHandler {
    client: PathOramClient<Channel>,
    n: i32,
    l: i32,
    z: i32,
    stash: HashMap<i32, i32>,
    pmap: Vec<i32>,
    num_leaves: i32,
}

impl PathORAMHandler {
    pub async fn new(client: PathOramClient<Channel>, z: i32) -> Self {
        PathORAMHandler {
            client,
            n: -1,
            l: -1,
            z,
            stash: HashMap::new(),
            pmap: Vec::new(),
            num_leaves: 0,
        }
    }

    async fn initialize_server(&mut self, num_layers: i32, bucket_size: i32) {
        let request = Request::new(SetupRequest {
            num_layers,
            bucket_size,
        });

        match self.client.setup(request).await {
            Ok(response) => {
                let setup_response: SetupResponse = response.into_inner();
                if setup_response.success {
                    println!("Server initialized.");
                } else {
                    println!("Initialization failed.");
                }
            }
            Err(e) => println!("Failed to initialize server: {:?}", e),
        }
    }

    pub async fn setup(&mut self, data: Vec<i32>, rng: &mut StdRng) {
        self.n = data.len() as i32;
        self.l = (self.n as f64).log2().ceil() as i32;
        self.num_leaves = if self.l > 0 { 2_i32.pow(self.l as u32) } else { 0 };

        self.initialize_server(self.l + 1, self.z).await;

        self.pmap = (0..self.n)
            .map(|_| rng.gen_range(0..self.num_leaves))
            .collect();

        for (a, value) in data.iter().enumerate() {
            self.write(a as i32, *value, rng).await;
        }
    }

    pub async fn update_stash(&mut self, _a: i32, x: i32) {
        for l in 0..=self.l {
            let index = self.get_index(x, l);
            let request = Request::new(ReadBlockRequest { index });
            match self.client.read_block(request).await {
                Ok(response) => {
                    let read_response: ReadBlockResponse = response.into_inner();
                    for block in read_response.blocks {
                        if block.index != -1 {
                            self.stash.insert(block.index, block.value);
                        }
                    }
                }
                Err(e) => println!("Failed to read block: {:?}", e),
            }
        }
    }

    pub async fn write_back_stash(&mut self, x: i32, _rng: &mut StdRng) {
        for l in (0..=self.l).rev() {
            let target_index = self.get_index(x, l);
            let valid_leaves: std::collections::HashSet<i32> = self.get_on_path_indices(x, l).collect();
            debug_println!("{:?}",valid_leaves);

            let mut write_back = Vec::new();
            for &a in self.stash.keys() {
                if valid_leaves.contains(&self.pmap[a as usize]) {
                    write_back.push(a);
                }
                if write_back.len() == self.z as usize {
                    break;
                }
            }

            let mut write_block_request = WriteBlockRequest { index: target_index, blocks: Vec::new() };
            for a in &write_back {
                write_block_request.blocks.push(Block {
                    value: self.stash[a],
                    index: *a,
                });
                self.stash.remove(a);
            }

            while write_block_request.blocks.len() < self.z as usize {
                write_block_request.blocks.push(Block { value: -1, index: -1 });
            }

            if let Err(e) = self.client.write_block(Request::new(write_block_request)).await {
                println!("Failed to write block: {:?}", e);
            }
        }
    }

    pub async fn read(&mut self, a: i32, rng: &mut StdRng) -> Option<i32> {
        let x = self.pmap[a as usize];
        self.pmap[a as usize] = rng.gen_range(0..self.num_leaves);
        self.update_stash(a, x).await;
        let out = self.stash.get(&a).cloned();
        self.write_back_stash(x, rng).await;
        let request = Request::new(PrintRequest {});
        debug_rpc_call!(self.client, request);
        out
    }

    pub async fn write(&mut self, a: i32, data: i32, rng: &mut StdRng) -> Option<i32> {
        debug_println!("\nwrite");
        let x = self.pmap[a as usize];
        self.pmap[a as usize] = rng.gen_range(0..self.num_leaves);
        self.update_stash(a, x).await;
        debug_println!("stash: {:?}",self.stash);
        debug_println!("pmap: {:?}",self.pmap);

        let out = self.stash.insert(a, data);

        debug_println!("a: {}; x: {}; pmap[{}]: {}", a, x, a, self.pmap[a as usize]);
        self.write_back_stash(x, rng).await;
        let request = Request::new(PrintRequest {});
        debug_rpc_call!(self.client, request);
        out
    }

    fn get_index(&self, x: i32, l: i32) -> i32 {
        let x = if self.l > 0 { (1 << self.l) + x } else { 1 };
        (x >> (self.l - l)) - 1
    }

    fn get_on_path_indices(&self, x: i32, l: i32) -> impl Iterator<Item = i32> {
        if l == self.l {
            return x..x + 1;
        }

        let l = self.l - l;
        let mask = (1 << l) - 1;
        let start = x & !mask;
        let end = x | mask;
        start..(end + 1)
    }
}

pub async fn run_client(n: i32, z: i32, rng: &mut StdRng) {
    let channel = Channel::from_static("http://localhost:50061")
        .connect()
        .await
        .unwrap();
    let client = PathOramClient::new(channel);
    let mut handler = PathORAMHandler::new(client, z).await;

    let data: Vec<i32> = (0..n).collect();
    handler.setup(data, rng).await;

    // let mut i = 0;
    // let mut start = Instant::now();

    // while i < 5 {
    //     if i % 1_000 == 0 && i > 0 {
    //         let elapsed = start.elapsed().as_secs_f64();
    //         println!("{i} iterations, time for last 1,000: {:.4} seconds", elapsed);
    //         start = Instant::now();
    //     }
    //     handler.read(rng.gen_range(0..n), rng).await;
    //     i += 1;
    // }
}

#[tokio::main]
async fn main() {
    let args = Args::parse();
    let mut rng = StdRng::seed_from_u64(11);
    let n = 1 << args.n;

    run_client(n, args.z, &mut rng).await;
}
