use std::ffi::c_void;
use std::mem::ManuallyDrop;

use anyhow::{Context, Result, bail};
use windows::Win32::Foundation::{CloseHandle, HANDLE, WAIT_OBJECT_0};
use windows::Win32::Graphics::Direct3D::Fxc::{D3DCOMPILE_OPTIMIZATION_LEVEL3, D3DCompile};
use windows::Win32::Graphics::Direct3D::ID3DBlob;
use windows::Win32::Graphics::Direct3D12::*;
use windows::Win32::Graphics::Dxgi::Common::{DXGI_FORMAT_UNKNOWN, DXGI_SAMPLE_DESC};
use windows::Win32::System::Threading::{CreateEventW, INFINITE, WaitForSingleObject};
use windows::core::{Interface, PCSTR};

pub struct Gpu {
    pub device: ID3D12Device,
    pub queue: ID3D12CommandQueue,
    allocator: ID3D12CommandAllocator,
    pub list: ID3D12GraphicsCommandList,
    list_type: D3D12_COMMAND_LIST_TYPE,
    fence: ID3D12Fence,
    fence_event: HANDLE,
    fence_value: u64,
    pub root_signature: ID3D12RootSignature,
}

impl Drop for Gpu {
    fn drop(&mut self) {
        let _ = unsafe { CloseHandle(self.fence_event) };
    }
}

fn blob_bytes(blob: &ID3DBlob) -> &[u8] {
    unsafe { std::slice::from_raw_parts(blob.GetBufferPointer().cast::<u8>(), blob.GetBufferSize()) }
}

fn root_signature(device: &ID3D12Device) -> Result<ID3D12RootSignature> {
    let parameters = [
        D3D12_ROOT_PARAMETER {
            ParameterType: D3D12_ROOT_PARAMETER_TYPE_32BIT_CONSTANTS,
            Anonymous: D3D12_ROOT_PARAMETER_0 {
                Constants: D3D12_ROOT_CONSTANTS {
                    ShaderRegister: 0,
                    RegisterSpace: 0,
                    Num32BitValues: 8,
                },
            },
            ShaderVisibility: D3D12_SHADER_VISIBILITY_ALL,
        },
        D3D12_ROOT_PARAMETER {
            ParameterType: D3D12_ROOT_PARAMETER_TYPE_SRV,
            Anonymous: D3D12_ROOT_PARAMETER_0 {
                Descriptor: D3D12_ROOT_DESCRIPTOR {
                    ShaderRegister: 0,
                    RegisterSpace: 0,
                },
            },
            ShaderVisibility: D3D12_SHADER_VISIBILITY_ALL,
        },
        D3D12_ROOT_PARAMETER {
            ParameterType: D3D12_ROOT_PARAMETER_TYPE_UAV,
            Anonymous: D3D12_ROOT_PARAMETER_0 {
                Descriptor: D3D12_ROOT_DESCRIPTOR {
                    ShaderRegister: 0,
                    RegisterSpace: 0,
                },
            },
            ShaderVisibility: D3D12_SHADER_VISIBILITY_ALL,
        },
        D3D12_ROOT_PARAMETER {
            ParameterType: D3D12_ROOT_PARAMETER_TYPE_UAV,
            Anonymous: D3D12_ROOT_PARAMETER_0 {
                Descriptor: D3D12_ROOT_DESCRIPTOR {
                    ShaderRegister: 1,
                    RegisterSpace: 0,
                },
            },
            ShaderVisibility: D3D12_SHADER_VISIBILITY_ALL,
        },
    ];
    let desc = D3D12_ROOT_SIGNATURE_DESC {
        NumParameters: parameters.len() as u32,
        pParameters: parameters.as_ptr(),
        NumStaticSamplers: 0,
        pStaticSamplers: std::ptr::null(),
        Flags: D3D12_ROOT_SIGNATURE_FLAG_NONE,
    };
    let mut blob = None;
    let mut error = None;
    unsafe { D3D12SerializeRootSignature(&desc, D3D_ROOT_SIGNATURE_VERSION_1, &mut blob, Some(&mut error)) }
        .with_context(|| {
            error
                .as_ref()
                .map(|e| String::from_utf8_lossy(blob_bytes(e)).into_owned())
                .unwrap_or_default()
        })?;
    let blob = blob.context("empty root signature")?;
    Ok(unsafe { device.CreateRootSignature(0, blob_bytes(&blob)) }?)
}

impl Gpu {
    pub fn from_queue(queue: ID3D12CommandQueue) -> Result<Self> {
        let mut device: Option<ID3D12Device> = None;
        unsafe { queue.GetDevice(&mut device) }?;
        let device = device.context("queue has no device")?;
        let list_type = unsafe { queue.GetDesc() }.Type;
        let allocator: ID3D12CommandAllocator = unsafe { device.CreateCommandAllocator(list_type) }?;
        let list: ID3D12GraphicsCommandList = unsafe { device.CreateCommandList(0, list_type, &allocator, None) }?;
        unsafe { list.Close() }?;
        let fence: ID3D12Fence = unsafe { device.CreateFence(0, D3D12_FENCE_FLAG_NONE) }?;
        let fence_event = unsafe { CreateEventW(None, false, false, None) }?;
        let root_signature = root_signature(&device)?;
        Ok(Self {
            device,
            queue,
            allocator,
            list,
            list_type,
            fence,
            fence_event,
            fence_value: 0,
            root_signature,
        })
    }

    pub fn list_type(&self) -> D3D12_COMMAND_LIST_TYPE {
        self.list_type
    }

    pub fn buffer(
        &self,
        bytes: u64,
        heap: D3D12_HEAP_TYPE,
        flags: D3D12_RESOURCE_FLAGS,
        state: D3D12_RESOURCE_STATES,
    ) -> Result<ID3D12Resource> {
        let heap_properties = D3D12_HEAP_PROPERTIES {
            Type: heap,
            ..Default::default()
        };
        let desc = D3D12_RESOURCE_DESC {
            Dimension: D3D12_RESOURCE_DIMENSION_BUFFER,
            Alignment: 0,
            Width: bytes,
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
        resource.context("buffer creation returned nothing")
    }

    pub fn uav_buffer(&self, bytes: u64) -> Result<ID3D12Resource> {
        self.buffer(
            bytes,
            D3D12_HEAP_TYPE_DEFAULT,
            D3D12_RESOURCE_FLAG_ALLOW_UNORDERED_ACCESS,
            D3D12_RESOURCE_STATE_UNORDERED_ACCESS,
        )
    }

    pub fn compute_pipeline(&self, source: &str) -> Result<ID3D12PipelineState> {
        let mut code: Option<ID3DBlob> = None;
        let mut errors: Option<ID3DBlob> = None;
        let result = unsafe {
            D3DCompile(
                source.as_ptr().cast::<c_void>(),
                source.len(),
                PCSTR::null(),
                None,
                None,
                PCSTR(c"main".as_ptr().cast()),
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
                .unwrap_or_default();
            bail!("shader compilation failed: {error} {message}");
        }
        let code = code.context("no shader bytecode")?;
        let bytes = blob_bytes(&code);
        let desc = D3D12_COMPUTE_PIPELINE_STATE_DESC {
            pRootSignature: ManuallyDrop::new(Some(self.root_signature.clone())),
            CS: D3D12_SHADER_BYTECODE {
                pShaderBytecode: bytes.as_ptr().cast(),
                BytecodeLength: bytes.len(),
            },
            ..Default::default()
        };
        let pipeline = unsafe { self.device.CreateComputePipelineState(&desc) }.context("CreateComputePipelineState");
        drop(ManuallyDrop::into_inner(desc.pRootSignature));
        pipeline
    }

    pub fn begin(&self, reset_allocator: bool) -> Result<()> {
        if reset_allocator {
            unsafe { self.allocator.Reset() }?;
        }
        unsafe { self.list.Reset(&self.allocator, None) }?;
        unsafe { self.list.SetComputeRootSignature(&self.root_signature) };
        Ok(())
    }

    pub fn dispatch(
        &self,
        pipeline: &ID3D12PipelineState,
        constants: &[u32; 8],
        source: &ID3D12Resource,
        output: &ID3D12Resource,
        extra: &ID3D12Resource,
        groups: (u32, u32),
    ) {
        unsafe {
            self.list.SetPipelineState(pipeline);
            self.list
                .SetComputeRoot32BitConstants(0, 8, constants.as_ptr().cast(), 0);
            self.list
                .SetComputeRootShaderResourceView(1, source.GetGPUVirtualAddress());
            self.list
                .SetComputeRootUnorderedAccessView(2, output.GetGPUVirtualAddress());
            self.list
                .SetComputeRootUnorderedAccessView(3, extra.GetGPUVirtualAddress());
            self.list.Dispatch(groups.0, groups.1, 1);
        }
    }

    pub fn copy_to_readback(&self, source: &ID3D12Resource, readback: &ID3D12Resource) {
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

    pub fn submit(&self) -> Result<()> {
        unsafe { self.list.Close() }?;
        let list: ID3D12CommandList = self.list.cast()?;
        unsafe { self.queue.ExecuteCommandLists(&[Some(list)]) };
        Ok(())
    }

    pub fn wait(&mut self) -> Result<()> {
        self.fence_value += 1;
        unsafe { self.queue.Signal(&self.fence, self.fence_value) }?;
        if unsafe { self.fence.GetCompletedValue() } < self.fence_value {
            unsafe { self.fence.SetEventOnCompletion(self.fence_value, self.fence_event) }?;
            if unsafe { WaitForSingleObject(self.fence_event, INFINITE) } != WAIT_OBJECT_0 {
                bail!("fence wait failed");
            }
        }
        Ok(())
    }
}

pub struct Mapped {
    resource: ID3D12Resource,
    pointer: *mut u8,
    len: usize,
}

impl Mapped {
    pub fn new(resource: ID3D12Resource, len: usize) -> Result<Self> {
        let mut pointer = std::ptr::null_mut();
        unsafe { resource.Map(0, Some(&D3D12_RANGE { Begin: 0, End: 0 }), Some(&mut pointer)) }?;
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
