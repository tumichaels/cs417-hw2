use tonic::{transport::Server, Request, Response, Status};

use path_oram::path_oram_server::{PathOram, PathOramServer};
use path_oram::{
    SetupRequest, SetupResponse,
    ReadBlockRequest, ReadBlockResponse,
    WriteBlockRequest, WriteBlockResponse,
    PrintRequest, PrintResponse
};
use path_oram::Block;
use std::sync::RwLock;
use std::cmp;

pub mod path_oram {
    tonic::include_proto!("path_oram"); // The string specified here must match the proto package name
}

#[derive(Debug, Default)]
pub struct MyPathOram {
    // Add fields here as needed to manage server state
    data_store: RwLock<Vec<Vec<Block>>>,  // 2D vector to simulate data storage with buckets and blocks
    bucket_size: RwLock<i32>,
}

impl MyPathOram {
    pub fn new(num_buckets: Option<usize>, bucket_size: Option<i32>) -> Self {
        // Initialize data_store with empty blocks (value = -1, index = -1) for each bucket
        let num_buckets = num_buckets.unwrap_or(0);
        let bucket_size = bucket_size.unwrap_or(0);

        let empty_block = Block { value: -1, index: -1 };
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

        let empty_block = Block { value: -1, index: -1 };
        let new_data_store = vec![vec![empty_block; setup_request.bucket_size as usize]; num_buckets];

         // Acquire a write lock to modify data_store and bucket_size
         let mut data_store = self.data_store.write().map_err(|_| Status::internal("Lock failed"))?;
         *data_store = new_data_store;  // Replace the existing data_store with the new one
 
         let mut bucket_size = self.bucket_size.write().map_err(|_| Status::internal("Lock failed"))?;
         *bucket_size = setup_request.bucket_size;

        println!("Initialized with L={}; Z={}", setup_request.num_layers, setup_request.bucket_size);

        display_tree(&data_store);
        let response = SetupResponse { success: true };
        Ok(Response::new(response))
    }

    // ReadBlock method with read lock
    async fn read_block(
        &self,
        request: Request<ReadBlockRequest>,
    ) -> Result<Response<ReadBlockResponse>, Status> {
        let index = request.get_ref().index as usize;

        // Acquire a read lock on data_store
        let data_store = self.data_store.read().map_err(|_| Status::internal("Lock failed"))?;

        if index < data_store.len() {
            let blocks = data_store[index].clone(); // Clone blocks to return owned data

            let response = ReadBlockResponse {
                blocks, // Automatically converts Vec<Block> to gRPC-compatible format
            };

            return Ok(Response::new(response));
        }

        // Return NOT_FOUND if index is out of bounds
        Err(Status::not_found("Index not found"))
    }

    // WriteBlock method with write lock
    async fn write_block(
        &self,
        request: Request<WriteBlockRequest>,
    ) -> Result<Response<WriteBlockResponse>, Status> {
        let index = request.get_ref().index as usize;

        // Acquire a write lock on data_store
        let mut data_store = self.data_store.write().map_err(|_| Status::internal("Lock failed"))?;
        let bucket_size = *self.bucket_size.read().map_err(|_| Status::internal("Lock failed"))?;

        if index < data_store.len() {
            for (i, entry) in request.get_ref().blocks.iter().take(bucket_size as usize).enumerate() {
                data_store[index][i] = Block {
                    value: entry.value,
                    index: entry.index,
                };
            }

            let response = WriteBlockResponse {
                success: true,
                message: "Data written successfully".to_string(),
            };

            return Ok(Response::new(response));
        }

        // Return NOT_FOUND if index is out of bounds
        Err(Status::not_found("Index not found"))
    }

    // Print method with read lock
    async fn print(
        &self,
        _request: Request<PrintRequest>,
    ) -> Result<Response<PrintResponse>, Status> {
        // Acquire a read lock on data_store
        let data_store = self.data_store.read().map_err(|_| Status::internal("Lock failed"))?;

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
                bucket.iter()
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

        let stacked_lines: Vec<Vec<&str>> = stacked_values.iter()
            .map(|value| value.lines().collect())
            .collect();

        for line in 0..stacked_lines[0].len() {
            let line_content: String = stacked_lines.iter()
                .map(|stack| stack[line])
                .collect::<Vec<&str>>()
                .join(&join_padding);
            
            println!("{}{}", line_padding, line_content);
        }

        println!();
    }
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
  let address = "[::1]:50061".parse().unwrap();
  let path_oram = MyPathOram::default();
  println!("Path ORAM Server listening on {}", address);

  Server::builder().add_service(PathOramServer::new(path_oram))
    .serve(address)
    .await?;
  Ok(())
     
}