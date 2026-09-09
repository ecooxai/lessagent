//! Image geometry is derived from the encoded artifact, never the screen mode
//! or requested browser size. Shared by file uploads, reads and computer output.
use crate::{Result, err};
use base64::Engine;
use serde_json::{Value, json};

pub fn attach(result: &mut Value, bytes: &[u8], mime: &str) -> Result<()> {
    let format = match imagesize::image_type(bytes)? {
        imagesize::ImageType::Png => "png",
        imagesize::ImageType::Jpeg => "jpeg",
        imagesize::ImageType::Gif => "gif",
        imagesize::ImageType::Webp => "webp",
        _ => return Err(err("Unsupported image format")),
    };
    if mime != format!("image/{format}") {
        return Err(err("Image bytes and MIME type do not agree"));
    }
    let size = imagesize::blob_size(bytes)?;
    if size.width == 0 || size.height == 0 {
        return Err(err("Image dimensions must be positive"));
    }
    let mut metadata = json!({"format":format, "mimeType":mime, "width":size.width,
        "height":size.height, "bytes":bytes.len(), "units":"pixels",
        "coordinate_space":result["coordinate_space"].as_str().unwrap_or("image")});
    for key in ["logical_width", "logical_height", "window_id", "pid", "browser_chrome_captured"] {
        if let Some(value) = result.get(key) { metadata[key] = value.clone(); }
    }
    result["width"] = json!(size.width);
    result["height"] = json!(size.height);
    result["format"] = json!(format);
    result["mime"] = json!(mime);
    result["bytes"] = json!(bytes.len());
    if result.get("screen_width").is_some() || result.get("coordinate_space").is_some() {
        result["screen_width"] = json!(size.width);
        result["screen_height"] = json!(size.height);
    }
    result["image_metadata"] = metadata.clone();
    result["image"] = json!({"mime":mime,
        "data":base64::engine::general_purpose::STANDARD.encode(bytes),
        "format":format, "width":size.width, "height":size.height, "metadata":metadata});
    Ok(())
}

/// `_meta` is the interoperable MCP extension location. Top-level dimension
/// aliases also support adapters that pass image metadata through directly.
/// No invented fovea/crop is emitted; these artifacts are full image frames.
pub fn mcp_block(image: &Value) -> Value {
    let mut block = json!({"type":"image", "mimeType":image["mime"], "data":image["data"]});
    if let Some(metadata) = image.get("metadata") {
        block["_meta"] = json!({"lessagent/image":metadata});
        for key in ["width", "height", "format", "metadata"] {
            if let Some(value) = image.get(key) { block[key] = value.clone(); }
        }
    }
    block
}

#[cfg(test)]
mod tests {
    use super::*;
    const PIXEL: &str = "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mP8/x8AAwMCAO+jRZkAAAAASUVORK5CYII=";
    #[test]
    fn encoded_geometry_overrides_stale_dimensions_and_survives_mcp() {
        let bytes = base64::engine::general_purpose::STANDARD.decode(PIXEL).unwrap();
        let mut value = json!({"screen_width":4000,"screen_height":3000,"width":null,"height":null,
            "coordinate_space":"window","logical_width":0.5,"logical_height":0.5,"window_id":7});
        attach(&mut value, &bytes, "image/png").unwrap();
        assert_eq!(value["width"], 1);
        assert_eq!(value["height"], 1);
        assert_eq!(value["screen_width"], 1);
        assert_eq!(value["screen_height"], 1);
        assert_eq!(value["image_metadata"]["logical_width"], 0.5);
        let block = mcp_block(&value["image"]);
        assert_eq!(block["_meta"]["lessagent/image"], value["image_metadata"]);
        assert_eq!(block["width"], 1);
        assert_eq!(block["height"], 1);
        assert_eq!(block["format"], "png");
        assert!(block.get("fovea").is_none());
        assert_eq!(base64::engine::general_purpose::STANDARD.decode(block["data"].as_str().unwrap()).unwrap(), bytes);
    }
    #[test]
    fn invalid_or_mislabeled_images_do_not_create_metadata() {
        let bytes = base64::engine::general_purpose::STANDARD.decode(PIXEL).unwrap();
        for (bytes, mime) in [(bytes.as_slice(), "image/jpeg"), (b"not an image".as_slice(), "image/png")] {
            let mut value = json!({});
            assert!(attach(&mut value, bytes, mime).is_err());
            assert_eq!(value, json!({}));
        }
    }
}
