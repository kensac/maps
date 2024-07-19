use osmpbfreader::primitive_block_from_blob;

extern crate osmpbfreader;

fn main() {
    let filename = std::env::args_os().nth(1).unwrap();
    let path = std::path::Path::new(&filename);
    let r = std::fs::File::open(&path).unwrap();
    let mut pbf = osmpbfreader::OsmPbfReader::new(r);

    for block in pbf.blobs().map(|b| primitive_block_from_blob(&b.unwrap())) {
        let block = block.unwrap();
        for group in block.primitivegroup {
            for node in group.nodes {
                println!("Node: {:?}", node);
            }
            for way in group.ways {
                println!("Way: {:?}", way);
            }
            for relation in group.relations {
                println!("Relation: {:?}", relation);
            }
        }
    }
}
