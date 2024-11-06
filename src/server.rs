use tonic::{transport::Server, Request, Response, Status};

use clap::Parser;
use path_oram::path_oram_server::{PathOram, PathOramServer};
use path_oram::Block;
use path_oram::{
    PrintRequest, PrintResponse, ReadBlockRequest, ReadBlockResponse, SetupRequest, SetupResponse,
    WriteBlockRequest, WriteBlockResponse,
};
use std::cmp;
use std::sync::RwLock;

pub mod path_oram {
    tonic::include_proto!("path_oram"); // The string specified here must match the proto package name
}

#[derive(Debug, Default)]
pub struct MyPathOram {
    // Add fields here as needed to manage server state
    data_store: RwLock<Vec<Vec<Block>>>, // 2D vector to simulate data storage with buckets and blocks
    bucket_size: RwLock<i32>,
}

impl MyPathOram {
    pub fn new(num_buckets: Option<usize>, bucket_size: Option<i32>) -> Self {
        // Initialize data_store with empty blocks (value = -1, index = -1) for each bucket
        let num_buckets = num_buckets.unwrap_or(0);
        let bucket_size = bucket_size.unwrap_or(0);

        let empty_block = Block {
            value: -1,
            index: -1,
        };
        let data_store = vec![vec![empty_block; bucket_size as usize]; num_buckets];

        MyPathOram {
            data_store: RwLock::new(data_store),
            bucket_size: RwLock::new(bucket_size),
        }
    }
}

#[tonic::async_trait]
impl PathOram for MyPathOram {
    // Setup method with write lock
    async fn setup(
        &self,
        request: Request<SetupRequest>,
    ) -> Result<Response<SetupResponse>, Status> {
        let setup_request = request.get_ref();
        let num_buckets = (2_usize.pow(setup_request.num_layers as u32)) - 1;

        let empty_block = Block {
            value: -1,
            index: -1,
        };
        let new_data_store =
            vec![vec![empty_block; setup_request.bucket_size as usize]; num_buckets];

        // Acquire a write lock to modify data_store and bucket_size
        let mut data_store = self
            .data_store
            .write()
            .map_err(|_| Status::internal("Lock failed"))?;
        *data_store = new_data_store; // Replace the existing data_store with the new one

        let mut bucket_size = self
            .bucket_size
            .write()
            .map_err(|_| Status::internal("Lock failed"))?;
        *bucket_size = setup_request.bucket_size;

        println!(
            "Initialized with L={}; Z={}",
            setup_request.num_layers, setup_request.bucket_size
        );

        // display_tree(&data_store);
        let response = SetupResponse { success: true };
        Ok(Response::new(response))
    }

    async fn read_block(
        &self,
        request: Request<ReadBlockRequest>,
    ) -> Result<Response<ReadBlockResponse>, Status> {
        let indices = &request.get_ref().indices;

        // Acquire a read lock on data_store
        let data_store = self
            .data_store
            .read()
            .map_err(|_| Status::internal("Lock failed"))?;

        // Gather blocks for each index in the list
        let mut blocks = Vec::new();
        for &index in indices {
            if let Some(data_blocks) = data_store.get(index as usize) {
                blocks.extend(data_blocks.clone()); // Collect blocks from each index
            } else {
                return Err(Status::not_found(format!("Index {} not found", index)));
            }
        }

        let response = ReadBlockResponse { blocks };

        Ok(Response::new(response))
    }

    async fn write_block(
        &self,
        request: Request<WriteBlockRequest>,
    ) -> Result<Response<WriteBlockResponse>, Status> {
        let WriteBlockRequest { indices, blocks } = request.into_inner();
        let mut block_iter = blocks.into_iter(); // Consume `blocks` into an iterator

        // Acquire a write lock on data_store
        let mut data_store = self
            .data_store
            .write()
            .map_err(|_| Status::internal("Lock failed"))?;
        let bucket_size = *self
            .bucket_size
            .read()
            .map_err(|_| Status::internal("Lock failed"))?;

        for &index in &indices {
            if index as usize >= data_store.len() {
                return Err(Status::not_found(format!("Index {} not found", index)));
            }

            // Write blocks to the specified index, respecting the bucket size
            for i in 0..bucket_size as usize {
                let entry = block_iter
                    .next()
                    .expect("There should always be enough blocks");

                data_store[index as usize][i] = Block {
                    value: entry.value,
                    index: entry.index,
                };
            }
        }

        let response = WriteBlockResponse { success: true };

        Ok(Response::new(response))
    }

    // Print method with read lock
    async fn print(
        &self,
        _request: Request<PrintRequest>,
    ) -> Result<Response<PrintResponse>, Status> {
        // Acquire a read lock on data_store
        let data_store = self
            .data_store
            .read()
            .map_err(|_| Status::internal("Lock failed"))?;

        // Call the display_tree function to print the data structure
        display_tree(&data_store);

        Ok(Response::new(PrintResponse { success: true }))
    }
}

// Utility function to display `data_store` as an implicit binary tree.
pub fn display_tree(data_store: &Vec<Vec<Block>>) {
    if data_store.is_empty() {
        println!("Tree is empty.");
        return;
    }

    let num_buckets = data_store.len();
    let height = (num_buckets as f64 + 1.0).log2().ceil() as usize;
    let max_width = 2_usize.pow((height - 1) as u32);

    for level in 0..height {
        let level_padding = (max_width / 2_usize.pow(level as u32)) - 1;
        let start_index = 2_usize.pow(level as u32) - 1;
        let end_index = cmp::min(start_index + 2_usize.pow(level as u32), num_buckets);

        let stacked_values: Vec<String> = (start_index..end_index)
            .filter_map(|i| data_store.get(i))
            .map(|bucket| {
                bucket
                    .iter()
                    .map(|block| {
                        if block.index == -1 {
                            "(_,_)".to_string()
                        } else {
                            format!("({},{})", block.value, block.index)
                        }
                    })
                    .collect::<Vec<String>>()
                    .join("\n")
            })
            .collect();

        let line_padding = " ".repeat(level_padding * 3);
        let join_padding = " ".repeat((level_padding * 2 * 3) + 1);

        let stacked_lines: Vec<Vec<&str>> = stacked_values
            .iter()
            .map(|value| value.lines().collect())
            .collect();

        for line in 0..stacked_lines[0].len() {
            let line_content: String = stacked_lines
                .iter()
                .map(|stack| stack[line])
                .collect::<Vec<&str>>()
                .join(&join_padding);

            println!("{}{}", line_padding, line_content);
        }

        println!();
    }
}

// CLI argument parser using `clap`
#[derive(Parser)]
struct Args {
    /// Port for the server to listen on
    #[arg(short, long, default_value = "50061")]
    port: u16,
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args = Args::parse();
    let address = format!("[::1]:{}", args.port).parse()?;
    let path_oram = MyPathOram::default();
    println!("Path ORAM Server listening on {}", address);

    Server::builder()
        .add_service(PathOramServer::new(path_oram))
        .serve(address)
        .await?;
    Ok(())
}
