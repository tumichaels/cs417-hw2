use clap::Parser;
use path_oram::{
    path_oram_client::PathOramClient, Block, PrintRequest, ReadBlockRequest, ReadBlockResponse,
    SetupRequest, SetupResponse, WriteBlockRequest,
};
use rand::rngs::StdRng;
use rand::{Rng, SeedableRng};
use std::collections::HashMap;
use std::fs::OpenOptions;
use std::io::Write;
use tokio::runtime::Runtime;
use tokio::time::Instant;
use tonic::transport::Channel;
use tonic::Request;

pub mod path_oram {
    tonic::include_proto!("path_oram");
}

#[derive(Parser, Debug)]
#[command(name = "Path ORAM Client", about = "Path ORAM gRPC Client in Rust")]
struct Args {
    #[arg(long)]
    n: i32,
    #[arg(long)]
    z: i32,
    #[arg(long)]
    b: i32,
    /// Port for the server to listen on
    #[arg(short, long, default_value = "50061")]
    port: u16,
}

macro_rules! debug_rpc_call {
    ($client:expr, $rt:expr) => {
        if cfg!(debug_assertions) {
            let request = Request::new(PrintRequest {});
            $rt.block_on(async {
                if let Err(e) = $client.print(request).await {
                    println!("Debug RPC call failed: {:?}", e);
                }
            });
        }
    };
}

macro_rules! debug_println {
    ($($arg:tt)*) => (if ::std::cfg!(debug_assertions) { ::std::println!($($arg)*); })
}

pub struct PathORAMHandler<'a> {
    client: PathOramClient<Channel>,
    n: i32,
    l: i32,
    z: i32,
    stash: HashMap<i32, i32>,
    pmap: Vec<i32>,
    num_leaves: i32,
    rt: &'a Runtime, // Single runtime for all async calls
    rng: StdRng,     // RNG as a struct member
}

impl<'a> PathORAMHandler<'a> {
    pub fn new(client: PathOramClient<Channel>, z: i32, rt: &'a Runtime, rng_seed: u64) -> Self {
        PathORAMHandler {
            client,
            n: -1,
            l: -1,
            z,
            stash: HashMap::new(),
            pmap: Vec::new(),
            num_leaves: 0,
            rt,
            rng: StdRng::seed_from_u64(rng_seed),
        }
    }

    pub fn initialize_server(&mut self, num_layers: i32, bucket_size: i32) {
        let request = Request::new(SetupRequest {
            num_layers,
            bucket_size,
        });

        let result = self.rt.block_on(self.client.setup(request));
        match result {
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

    pub fn setup(&mut self, data: Vec<i32>) {
        self.n = data.len() as i32;
        self.l = (self.n as f64).log2().ceil() as i32;
        self.num_leaves = if self.l > 0 {
            2_i32.pow(self.l as u32)
        } else {
            0
        };

        self.initialize_server(self.l + 1, self.z);

        self.pmap = (0..self.n)
            .map(|_| self.rng.gen_range(0..self.num_leaves))
            .collect();

        for (a, value) in data.iter().enumerate() {
            self.write(a as i32, *value);
        }
        println!("Data written to server");
    }

    pub fn update_stash(&mut self, _a: i32, x: i32) {
        let mut indices = Vec::new();

        // Collect all indices for the RPC call
        for l in 0..=self.l {
            let index = self.get_index(x, l);
            indices.push(index);
        }

        // Create and send a single ReadBlockRequest with the list of indices
        let request = Request::new(ReadBlockRequest { indices });

        let result = self.rt.block_on(self.client.read_block(request));
        match result {
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

    pub fn write_back_stash(&mut self, x: i32) {
        let mut write_block_request = WriteBlockRequest {
            indices: Vec::new(),
            blocks: Vec::new(),
        };
    
        for l in (0..=self.l).rev() {
            let target_index = self.get_index(x, l);
            let valid_leaves: std::collections::HashSet<i32> = self.get_on_path_indices(x, l).collect();
            debug_println!("{:?}", valid_leaves);
    
            let mut write_back = Vec::new();
            for &a in self.stash.keys() {
                if valid_leaves.contains(&self.pmap[a as usize]) {
                    write_back.push(a);
                }
                if write_back.len() == self.z as usize {
                    break;
                }
            }
    
            // Add the target index to the request
            write_block_request.indices.push(target_index);
    
            // Collect blocks for this index, filling with dummy blocks if needed
            let mut blocks_for_index = Vec::new();
            for a in &write_back {
                blocks_for_index.push(Block {
                    value: self.stash[a],
                    index: *a,
                });
                self.stash.remove(a);
            }
    
            while blocks_for_index.len() < self.z as usize {
                blocks_for_index.push(Block {
                    value: -1,
                    index: -1,
                });
            }

            // Append blocks for this index to the main blocks list
            write_block_request.blocks.extend(blocks_for_index);
        }

        debug_println!("write request: {:?}", write_block_request);
    
        // Send the batched write request
        if let Err(e) = self
            .rt
            .block_on(self.client.write_block(Request::new(write_block_request)))
        {
            println!("Failed to write block: {:?}", e);
        }
    }
    
    

    pub fn read(&mut self, a: i32) -> Option<i32> {
        debug_println!("\nread");
        let x = self.pmap[a as usize];
        self.pmap[a as usize] = self.rng.gen_range(0..self.num_leaves);
        self.update_stash(a, x);
        debug_println!("stash: {:?}", self.stash);
        debug_println!("pmap: {:?}", self.pmap);

        let out = self.stash.get(&a).cloned();
        debug_println!("a: {}; x: {}; pmap[{}]: {}", a, x, a, self.pmap[a as usize]);
        self.write_back_stash(x);

        debug_rpc_call!(self.client, self.rt);

        out
    }

    pub fn write(&mut self, a: i32, data: i32) -> Option<i32> {
        debug_println!("\nwrite");
        let x = self.pmap[a as usize];
        self.pmap[a as usize] = self.rng.gen_range(0..self.num_leaves);
        self.update_stash(a, x);
        debug_println!("stash: {:?}", self.stash);
        debug_println!("pmap: {:?}", self.pmap);

        let out = self.stash.insert(a, data);

        debug_println!("a: {}; x: {}; pmap[{}]: {}", a, x, a, self.pmap[a as usize]);
        self.write_back_stash(x);

        debug_rpc_call!(self.client, self.rt);

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

fn run_client(port: u16, n: i32, z: i32, rng_seed: u64) {
    let exp = n;
    let n = 1 << exp;
    let rt = Runtime::new().unwrap();

    let channel = rt
        .block_on(Channel::from_shared(format!("http://localhost:{}", port)).unwrap().connect())
        .unwrap();
    let client = PathOramClient::new(channel);
    let mut handler = PathORAMHandler::new(client, z, &rt, rng_seed);

    let data: Vec<i32> = (0..n).collect();
    let start = Instant::now();
    handler.setup(data);
    let elapsed = start.elapsed().as_secs_f64();
    println!("\nsetup time taken: {:.4} seconds", elapsed);

    run_experiment(handler, n, z, rng_seed);
}

fn run_experiment(mut handler: PathORAMHandler<'_>, n: i32, z: i32, rng_seed: u64) {
    let mut start = Instant::now();
    for i in 0..3_000_000 {
        handler.read(i % n); // Use modulo to stay within the range of `n`

        if i % 10_000 == 0 && i > 0 {
            let elapsed = start.elapsed().as_secs_f64();
            println!(
                "Warmup: {} reads completed, time for last 10,000: {:.4} seconds",
                i, elapsed
            );
            start = Instant::now(); // Reset timer
        }
    }

    let mut stash_file = OpenOptions::new()
        .create(true)
        .append(false)
        .open(format!("stash_sizes_n={}_z={}_b={}.txt", n, z, rng_seed))
        .expect("Unable to open file");

    // Perform 7 million read operations
    let mut start = Instant::now();
    for i in 0..7_000_000 {
        handler.read(i % n); // Use modulo to stay within the range of `n`

        // Write stash size to the file
        writeln!(stash_file, "{}", handler.stash.len()).expect("Unable to write to file");

        // Display time taken for every 10,000 operations
        if i % 10 == 0 && i > 0 {
            let elapsed = start.elapsed().as_secs_f64();
            println!(
                "test: {} reads completed, time for last 10,000: {:.4} seconds",
                i, elapsed
            );
            stash_file.flush().expect("Unable to flush file"); // Flush to ensure data is saved
            start = Instant::now(); // Reset timer
        }
    }
}

fn main() {
    let args = Args::parse();
    let rng_seed = 11;

    run_client(args.port, args.n, args.z, rng_seed);
}