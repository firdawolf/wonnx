pub mod boilerplate;
pub mod compute;
use std::error;
pub mod compiler;
pub mod dimensions;
pub mod onnx;
pub mod resource;
pub mod utils;
use log::debug;
use protobuf::{self, Message};
use std::collections::HashMap;
// Change the alias to `Box<error::Error>`.
type Result<T> = std::result::Result<T, Box<dyn error::Error>>;
/// Creates a new session connected to the GPU.
///
/// Generate a session that will translate the onnx format into WGSL instructions.
///
/// # Examples
///
/// Basic usage:
///
/// ```ignore
/// let mut session = Session::from_path("path/to/model.onnx").await.unwrap();
/// ```
pub struct Session {
    pub device: wgpu::Device,
    pub queue: wgpu::Queue,
    pub model: onnx::ModelProto,
    pub inner_infos: HashMap<String, InnerInfo>,
}

impl Session {
    pub async fn from_path(path: &str) -> Result<Session> {
        let (device, queue) = resource::request_device_queue().await;

        let model = onnx::ModelProto::parse_from_bytes(
            &std::fs::read(path).expect("ONNX Model path not found."),
        )
        .expect("Could not deserialize the Model");

        let inner_infos = Session::load_initializers(&device, &model).unwrap();

        Ok(Session {
            device,
            queue,
            model,
            inner_infos,
        })
    }

    pub async fn from_model(model: onnx::ModelProto) -> Result<Session> {
        let (device, queue) = resource::request_device_queue().await;

        let inner_infos = Session::load_initializers(&device, &model).unwrap();

        Ok(Session {
            device,
            queue,
            model,
            inner_infos,
        })
    }

    pub fn load_initializers(
        device: &wgpu::Device,
        model: &onnx::ModelProto,
    ) -> Result<HashMap<std::string::String, InnerInfo>> {
        let mut inner_infos = HashMap::new();
        let initializers = model.get_graph().get_initializer();
        for initializer in initializers.iter() {
            let input = initializer.get_name();

            let initiated_data = initializers
                .iter()
                .find(|x| x.get_name() == input)
                .expect(format!("Did not find initializer for input: {}", input).as_str());

            let initiated_data_dims = initiated_data.get_dims().to_vec();
            inner_infos.insert(
                input.to_string(),
                InnerInfo {
                    buffer: resource::create_buffer_init(
                        &device,
                        initiated_data.get_float_data(),
                        input,
                    ),
                    dims: initiated_data_dims.clone(),
                    inner_type: crate::compute::InnerType::ArrayVector,
                },
            );
        }

        Ok(inner_infos)
    }

    pub async fn run(&mut self, input_data: HashMap<String, (&[f32], &[i64])>) -> Option<Vec<f32>> {
        let graph = self.model.get_graph();
        let device = &self.device;
        let queue = &self.queue;

        let inner_infos =
            dimensions::generate_buffer(input_data, graph, device, &mut self.inner_infos);

        compute::wrapper(device, queue, graph, &inner_infos).unwrap();

        let outputs = graph.get_output();
        // TODO: Define behavior for multi output.
        let buffer_slice = inner_infos
            .get(outputs[0].get_name())
            .unwrap()
            .buffer
            .slice(..);
        let buffer_future = buffer_slice.map_async(wgpu::MapMode::Read);
        device.poll(wgpu::Maintain::Wait);

        // OUTPUT

        if let Ok(()) = buffer_future.await {
            // Gets contents of buffer
            let data = buffer_slice.get_mapped_range();
            // Since contents are got in bytes, this converts these bytes back to f32
            let result = bytemuck::cast_slice(&data).to_vec();

            //            drop(data);

            Some(result)
        } else {
            panic!("failed to run compute on gpu!")
        }
    }
}

pub struct InnerInfo {
    buffer: wgpu::Buffer,
    dims: Vec<i64>,
    inner_type: compute::InnerType,
}

pub fn get_attribute<'a>(
    attribute: &'a str,
    defaults: Option<&'a onnx::AttributeProto>,
    node: &'a onnx::NodeProto,
) -> &'a onnx::AttributeProto {
    match defaults {
        Some(default) => node
            .get_attribute()
            .iter()
            .find(|attr| attr.get_name() == attribute)
            .unwrap_or(&default),
        None => node
            .get_attribute()
            .iter()
            .find(|attr| attr.get_name() == attribute)
            .expect(
                format!(
                    "Did not find required attribute: {}, for node: {}",
                    attribute,
                    node.get_name()
                )
                .as_str(),
            ),
    }
}
