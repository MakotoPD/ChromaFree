use std::ffi::c_void;
use std::mem::ManuallyDrop;

use windows::Win32::AI::MachineLearning::DirectML::{DML_CREATE_DEVICE_FLAG_NONE, DMLCreateDevice, IDMLDevice};
use windows::Win32::Foundation::{CloseHandle, HANDLE, WAIT_OBJECT_0};
use windows::Win32::Graphics::Direct3D::Fxc::{D3DCOMPILE_OPTIMIZATION_LEVEL3, D3DCompile};
use windows::Win32::Graphics::Direct3D::{D3D_FEATURE_LEVEL_11_0, ID3DBlob};
use windows::Win32::Graphics::Direct3D12::*;
use windows::Win32::Graphics::Dxgi::Common::{DXGI_FORMAT_UNKNOWN, DXGI_SAMPLE_DESC};
use windows::Win32::Graphics::Dxgi::{CreateDXGIFactory1, IDXGIFactory1};
use windows::Win32::System::Threading::{CreateEventW, INFINITE, WaitForSingleObject};
use windows::core::{Interface, PCSTR};

use crate::error::GpuError;

pub const ROOT_CONSTANTS: usize = 16;
const UNORDERED_ACCESS_SLOTS: usize = 4;

pub struct GpuDevice {
    pub(crate) device: ID3D12Device,
    pub(crate) queue: ID3D12CommandQueue,
    pub(crate) dml: IDMLDevice,
    allocator: ID3D12CommandAllocator,
    list: ID3D12GraphicsCommandList,
    fence: ID3D12Fence,
    fence_event: HANDLE,
    fence_value: u64,
    root_signature: ID3D12RootSignature,
    adapter_name: String,
}

impl Drop for GpuDevice {
    fn drop(&mut self) {
        let _ = unsafe { CloseHandle(self.fence_event) };
    }
}

fn blob_bytes(blob: &ID3DBlob) -> &[u8] {
    unsafe { std::slice::from_raw_parts(blob.GetBufferPointer().cast::<u8>(), blob.GetBufferSize()) }
}

fn descriptor_parameter(kind: D3D12_ROOT_PARAMETER_TYPE, register: u32) -> D3D12_ROOT_PARAMETER {
    D3D12_ROOT_PARAMETER {
        ParameterType: kind,
        Anonymous: D3D12_ROOT_PARAMETER_0 {
            Descriptor: D3D12_ROOT_DESCRIPTOR {
                ShaderRegister: register,
                RegisterSpace: 0,
            },
        },
        ShaderVisibility: D3D12_SHADER_VISIBILITY_ALL,
    }
}

fn root_signature(device: &ID3D12Device) -> Result<ID3D12RootSignature, GpuError> {
    let mut parameters = vec![
        D3D12_ROOT_PARAMETER {
            ParameterType: D3D12_ROOT_PARAMETER_TYPE_32BIT_CONSTANTS,
            Anonymous: D3D12_ROOT_PARAMETER_0 {
                Constants: D3D12_ROOT_CONSTANTS {
                    ShaderRegister: 0,
                    RegisterSpace: 0,
                    Num32BitValues: ROOT_CONSTANTS as u32,
                },
            },
            ShaderVisibility: D3D12_SHADER_VISIBILITY_ALL,
        },
        descriptor_parameter(D3D12_ROOT_PARAMETER_TYPE_SRV, 0),
    ];
    parameters
        .extend((0..UNORDERED_ACCESS_SLOTS as u32).map(|r| descriptor_parameter(D3D12_ROOT_PARAMETER_TYPE_UAV, r)));
    let desc = D3D12_ROOT_SIGNATURE_DESC {
        NumParameters: parameters.len() as u32,
        pParameters: parameters.as_ptr(),
        NumStaticSamplers: 0,
        pStaticSamplers: std::ptr::null(),
        Flags: D3D12_ROOT_SIGNATURE_FLAG_NONE,
    };
    let mut blob = None;
    let mut error = None;
    if let Err(e) =
        unsafe { D3D12SerializeRootSignature(&desc, D3D_ROOT_SIGNATURE_VERSION_1, &mut blob, Some(&mut error)) }
    {
        let message = error
            .as_ref()
            .map(|b| String::from_utf8_lossy(blob_bytes(b)).into_owned());
        return Err(GpuError::Shader {
            entry: "root signature".into(),
            message: message.unwrap_or_else(|| e.to_string()),
        });
    }
    let blob = blob.ok_or_else(|| GpuError::Shader {
        entry: "root signature".into(),
        message: "empty blob".into(),
    })?;
    Ok(unsafe { device.CreateRootSignature(0, blob_bytes(&blob)) }?)
}

impl GpuDevice {
    pub fn new(adapter_index: u32) -> Result<Self, GpuError> {
        bgcam_core::segmentation::bundled_directml();
        let factory: IDXGIFactory1 = unsafe { CreateDXGIFactory1() }?;
        let adapter =
            unsafe { factory.EnumAdapters1(adapter_index) }.map_err(|_| GpuError::AdapterNotFound(adapter_index))?;
        let description = unsafe { adapter.GetDesc1() }?;
        let name_len = description
            .Description
            .iter()
            .position(|&c| c == 0)
            .unwrap_or(description.Description.len());
        let adapter_name = String::from_utf16_lossy(&description.Description[..name_len]);

        let mut device: Option<ID3D12Device> = None;
        unsafe { D3D12CreateDevice(&adapter, D3D_FEATURE_LEVEL_11_0, &mut device) }?;
        let device = device.ok_or(GpuError::AdapterNotFound(adapter_index))?;
        let queue: ID3D12CommandQueue = unsafe {
            device.CreateCommandQueue(&D3D12_COMMAND_QUEUE_DESC {
                Type: D3D12_COMMAND_LIST_TYPE_DIRECT,
                ..Default::default()
            })
        }?;
        let mut dml: Option<IDMLDevice> = None;
        unsafe { DMLCreateDevice(&device, DML_CREATE_DEVICE_FLAG_NONE, &mut dml) }
            .map_err(|e| GpuError::DirectMl(e.to_string()))?;
        let dml = dml.ok_or_else(|| GpuError::DirectMl("DMLCreateDevice returned nothing".into()))?;

        let allocator: ID3D12CommandAllocator =
            unsafe { device.CreateCommandAllocator(D3D12_COMMAND_LIST_TYPE_DIRECT) }?;
        let list: ID3D12GraphicsCommandList =
            unsafe { device.CreateCommandList(0, D3D12_COMMAND_LIST_TYPE_DIRECT, &allocator, None) }?;
        unsafe { list.Close() }?;
        let fence: ID3D12Fence = unsafe { device.CreateFence(0, D3D12_FENCE_FLAG_NONE) }?;
        let fence_event = unsafe { CreateEventW(None, false, false, None) }?;
        let root_signature = root_signature(&device)?;
        Ok(Self {
            device,
            queue,
            dml,
            allocator,
            list,
            fence,
            fence_event,
            fence_value: 0,
            root_signature,
            adapter_name,
        })
    }

    pub fn adapter_name(&self) -> &str {
        &self.adapter_name
    }

    pub(crate) fn buffer(
        &self,
        bytes: usize,
        heap: D3D12_HEAP_TYPE,
        flags: D3D12_RESOURCE_FLAGS,
        state: D3D12_RESOURCE_STATES,
    ) -> Result<ID3D12Resource, GpuError> {
        let heap_properties = D3D12_HEAP_PROPERTIES {
            Type: heap,
            ..Default::default()
        };
        let desc = D3D12_RESOURCE_DESC {
            Dimension: D3D12_RESOURCE_DIMENSION_BUFFER,
            Alignment: 0,
            Width: bytes.div_ceil(4).max(1) as u64 * 4,
            Height: 1,
            DepthOrArraySize: 1,
            MipLevels: 1,
            Format: DXGI_FORMAT_UNKNOWN,
            SampleDesc: DXGI_SAMPLE_DESC { Count: 1, Quality: 0 },
            Layout: D3D12_TEXTURE_LAYOUT_ROW_MAJOR,
            Flags: flags,
        };
        let mut resource: Option<ID3D12Resource> = None;
        unsafe {
            self.device.CreateCommittedResource(
                &heap_properties,
                D3D12_HEAP_FLAG_NONE,
                &desc,
                state,
                None,
                &mut resource,
            )
        }?;
        resource.ok_or_else(|| GpuError::DirectMl("buffer creation returned nothing".into()))
    }

    pub(crate) fn storage_buffer(&self, bytes: usize) -> Result<ID3D12Resource, GpuError> {
        self.buffer(
            bytes,
            D3D12_HEAP_TYPE_DEFAULT,
            D3D12_RESOURCE_FLAG_ALLOW_UNORDERED_ACCESS,
            D3D12_RESOURCE_STATE_UNORDERED_ACCESS,
        )
    }

    pub(crate) fn upload_buffer(&self, bytes: usize) -> Result<MappedBuffer, GpuError> {
        let resource = self.buffer(
            bytes,
            D3D12_HEAP_TYPE_UPLOAD,
            D3D12_RESOURCE_FLAG_NONE,
            D3D12_RESOURCE_STATE_GENERIC_READ,
        )?;
        MappedBuffer::new(resource, bytes)
    }

    pub(crate) fn readback_buffer(&self, bytes: usize) -> Result<MappedBuffer, GpuError> {
        let resource = self.buffer(
            bytes,
            D3D12_HEAP_TYPE_READBACK,
            D3D12_RESOURCE_FLAG_NONE,
            D3D12_RESOURCE_STATE_COPY_DEST,
        )?;
        MappedBuffer::new(resource, bytes)
    }

    pub(crate) fn compile(&self, source: &str, entry: &str) -> Result<ID3D12PipelineState, GpuError> {
        let mut code: Option<ID3DBlob> = None;
        let mut errors: Option<ID3DBlob> = None;
        let entry_name = std::ffi::CString::new(entry).map_err(|e| GpuError::Shader {
            entry: entry.into(),
            message: e.to_string(),
        })?;
        let result = unsafe {
            D3DCompile(
                source.as_ptr().cast::<c_void>(),
                source.len(),
                PCSTR::null(),
                None,
                None,
                PCSTR(entry_name.as_ptr().cast()),
                PCSTR(c"cs_5_0".as_ptr().cast()),
                D3DCOMPILE_OPTIMIZATION_LEVEL3,
                0,
                &mut code,
                Some(&mut errors),
            )
        };
        if let Err(error) = result {
            let message = errors
                .as_ref()
                .map(|e| String::from_utf8_lossy(blob_bytes(e)).into_owned())
                .unwrap_or_else(|| error.to_string());
            return Err(GpuError::Shader {
                entry: entry.into(),
                message,
            });
        }
        let code = code.ok_or_else(|| GpuError::Shader {
            entry: entry.into(),
            message: "no bytecode".into(),
        })?;
        let bytes = blob_bytes(&code);
        let desc = D3D12_COMPUTE_PIPELINE_STATE_DESC {
            pRootSignature: ManuallyDrop::new(Some(self.root_signature.clone())),
            CS: D3D12_SHADER_BYTECODE {
                pShaderBytecode: bytes.as_ptr().cast(),
                BytecodeLength: bytes.len(),
            },
            ..Default::default()
        };
        let pipeline = unsafe { self.device.CreateComputePipelineState(&desc) };
        drop(ManuallyDrop::into_inner(desc.pRootSignature));
        Ok(pipeline?)
    }

    pub(crate) fn begin(&self, new_frame: bool) -> Result<(), GpuError> {
        if new_frame {
            unsafe { self.allocator.Reset() }?;
        }
        unsafe { self.list.Reset(&self.allocator, None) }?;
        unsafe { self.list.SetComputeRootSignature(&self.root_signature) };
        Ok(())
    }

    pub(crate) fn dispatch(&self, pass: &Pass<'_>) {
        unsafe {
            self.list.SetPipelineState(pass.pipeline);
            self.list
                .SetComputeRoot32BitConstants(0, ROOT_CONSTANTS as u32, pass.constants.as_ptr().cast(), 0);
            self.list
                .SetComputeRootShaderResourceView(1, pass.shader_resource.GetGPUVirtualAddress());
            for (slot, resource) in pass.unordered.iter().enumerate() {
                self.list
                    .SetComputeRootUnorderedAccessView(2 + slot as u32, resource.GetGPUVirtualAddress());
            }
            self.list.Dispatch(pass.groups.0.max(1), pass.groups.1.max(1), 1);
        }
        self.wait_for_unordered_access(&pass.unordered);
    }

    fn wait_for_unordered_access(&self, resources: &[&ID3D12Resource; UNORDERED_ACCESS_SLOTS]) {
        let barriers = resources.map(|resource| D3D12_RESOURCE_BARRIER {
            Type: D3D12_RESOURCE_BARRIER_TYPE_UAV,
            Flags: D3D12_RESOURCE_BARRIER_FLAG_NONE,
            Anonymous: D3D12_RESOURCE_BARRIER_0 {
                UAV: ManuallyDrop::new(D3D12_RESOURCE_UAV_BARRIER {
                    pResource: ManuallyDrop::new(Some(resource.clone())),
                }),
            },
        });
        unsafe { self.list.ResourceBarrier(&barriers) };
        for barrier in barriers {
            let uav = ManuallyDrop::into_inner(unsafe { barrier.Anonymous.UAV });
            drop(ManuallyDrop::into_inner(uav.pResource));
        }
    }

    pub(crate) fn copy_to_readback(&self, source: &ID3D12Resource, readback: &ID3D12Resource) {
        let transition = |from, to| D3D12_RESOURCE_BARRIER {
            Type: D3D12_RESOURCE_BARRIER_TYPE_TRANSITION,
            Flags: D3D12_RESOURCE_BARRIER_FLAG_NONE,
            Anonymous: D3D12_RESOURCE_BARRIER_0 {
                Transition: ManuallyDrop::new(D3D12_RESOURCE_TRANSITION_BARRIER {
                    pResource: ManuallyDrop::new(Some(source.clone())),
                    Subresource: D3D12_RESOURCE_BARRIER_ALL_SUBRESOURCES,
                    StateBefore: from,
                    StateAfter: to,
                }),
            },
        };
        let to_copy = [transition(
            D3D12_RESOURCE_STATE_UNORDERED_ACCESS,
            D3D12_RESOURCE_STATE_COPY_SOURCE,
        )];
        let back = [transition(
            D3D12_RESOURCE_STATE_COPY_SOURCE,
            D3D12_RESOURCE_STATE_UNORDERED_ACCESS,
        )];
        unsafe {
            self.list.ResourceBarrier(&to_copy);
            self.list.CopyResource(readback, source);
            self.list.ResourceBarrier(&back);
        }
        for barrier in to_copy.into_iter().chain(back) {
            let transition = ManuallyDrop::into_inner(unsafe { barrier.Anonymous.Transition });
            drop(ManuallyDrop::into_inner(transition.pResource));
        }
    }

    pub(crate) fn submit(&self) -> Result<(), GpuError> {
        unsafe { self.list.Close() }?;
        let list: ID3D12CommandList = self.list.cast()?;
        unsafe { self.queue.ExecuteCommandLists(&[Some(list)]) };
        Ok(())
    }

    pub(crate) fn wait(&mut self) -> Result<(), GpuError> {
        self.fence_value += 1;
        unsafe { self.queue.Signal(&self.fence, self.fence_value) }?;
        if unsafe { self.fence.GetCompletedValue() } < self.fence_value {
            unsafe { self.fence.SetEventOnCompletion(self.fence_value, self.fence_event) }?;
            if unsafe { WaitForSingleObject(self.fence_event, INFINITE) } != WAIT_OBJECT_0 {
                return Err(GpuError::DirectMl("waiting for the GPU failed".into()));
            }
        }
        unsafe { self.device.GetDeviceRemovedReason() }.map_err(|e| GpuError::DeviceRemoved(e.to_string()))
    }
}

pub(crate) struct Pass<'a> {
    pub pipeline: &'a ID3D12PipelineState,
    pub constants: [u32; ROOT_CONSTANTS],
    pub shader_resource: &'a ID3D12Resource,
    pub unordered: [&'a ID3D12Resource; UNORDERED_ACCESS_SLOTS],
    pub groups: (u32, u32),
}

pub(crate) fn groups(width: u32, height: u32) -> (u32, u32) {
    (width.div_ceil(16), height.div_ceil(16))
}

pub(crate) struct MappedBuffer {
    resource: ID3D12Resource,
    pointer: *mut u8,
    len: usize,
}

unsafe impl Send for MappedBuffer {}

impl MappedBuffer {
    fn new(resource: ID3D12Resource, len: usize) -> Result<Self, GpuError> {
        let mut pointer = std::ptr::null_mut();
        unsafe { resource.Map(0, None, Some(&mut pointer)) }?;
        Ok(Self {
            resource,
            pointer: pointer.cast(),
            len,
        })
    }

    pub fn resource(&self) -> &ID3D12Resource {
        &self.resource
    }

    pub fn bytes(&self) -> &[u8] {
        unsafe { std::slice::from_raw_parts(self.pointer, self.len) }
    }

    pub fn bytes_mut(&mut self) -> &mut [u8] {
        unsafe { std::slice::from_raw_parts_mut(self.pointer, self.len) }
    }
}
