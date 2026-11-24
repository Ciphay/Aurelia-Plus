use std::io::{Read, Write};
use steam_vent_proto_common::{RpcMessage, RpcMethod};

include!(concat!(env!("OUT_DIR"), "/cloudconfig/mod.rs"));

pub use service_cloudconfigstore::{
    CCloudConfigStore_Download_Request, CCloudConfigStore_Download_Response,
    CCloudConfigStore_Entry, CCloudConfigStore_NamespaceData, CCloudConfigStore_NamespaceVersion,
    CCloudConfigStore_Upload_Request, CCloudConfigStore_Upload_Response,
};

macro_rules! rpc_message {
    ($t:ty) => {
        impl RpcMessage for $t {
            fn parse(reader: &mut dyn Read) -> ::protobuf::Result<Self> {
                <Self as ::protobuf::Message>::parse_from_reader(reader)
            }
            fn write(&self, writer: &mut dyn Write) -> ::protobuf::Result<()> {
                use ::protobuf::Message;
                self.write_to_writer(writer)
            }
            fn encode_size(&self) -> usize {
                use ::protobuf::Message;
                self.compute_size() as usize
            }
        }
    };
}

rpc_message!(CCloudConfigStore_Download_Request);
rpc_message!(CCloudConfigStore_Download_Response);
rpc_message!(CCloudConfigStore_Upload_Request);
rpc_message!(CCloudConfigStore_Upload_Response);

impl RpcMethod for CCloudConfigStore_Download_Request {
    const METHOD_NAME: &'static str = "CloudConfigStore.Download#1";
    type Response = CCloudConfigStore_Download_Response;
}

impl RpcMethod for CCloudConfigStore_Upload_Request {
    const METHOD_NAME: &'static str = "CloudConfigStore.Upload#1";
    type Response = CCloudConfigStore_Upload_Response;
}
