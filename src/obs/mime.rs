//! MIME type to file extension table used to name uploaded objects.

/// `(mime, extension)` pairs, as in linejs `obs/mime.ts`.
pub const MIME_TYPES: &[(&str, &str)] = &[
    ("audio/x-mpeg", "mpega"),
    ("application/postscript", "ps"),
    ("audio/x-aiff", "aiff"),
    ("application/x-aim", "aim"),
    ("image/x-jg", "art"),
    ("video/x-ms-asf", "asx"),
    ("audio/basic", "ulw"),
    ("video/x-msvideo", "avi"),
    ("video/x-rad-screenplay", "avx"),
    ("application/x-bcpio", "bcpio"),
    ("application/octet-stream", "exe"),
    ("image/bmp", "dib"),
    ("text/html", "html"),
    ("application/x-cdf", "cdf"),
    ("application/pkix-cert", "cer"),
    ("application/java", "class"),
    ("application/x-cpio", "cpio"),
    ("application/x-csh", "csh"),
    ("text/css", "css"),
    ("application/msword", "doc"),
    ("application/xml-dtd", "dtd"),
    ("video/x-dv", "dv"),
    ("application/x-dvi", "dvi"),
    ("application/vnd.ms-fontobject", "eot"),
    ("text/x-setext", "etx"),
    ("image/gif", "gif"),
    ("application/x-gtar", "gtar"),
    ("application/x-gzip", "gz"),
    ("application/x-hdf", "hdf"),
    ("application/mac-binhex40", "hqx"),
    ("text/x-component", "htc"),
    ("image/ief", "ief"),
    ("text/vnd.sun.j2me.app-descriptor", "jad"),
    ("application/java-archive", "jar"),
    ("text/x-java-source", "java"),
    ("application/x-java-jnlp-file", "jnlp"),
    ("image/jpeg", "jpg"),
    ("application/javascript", "js"),
    ("text/plain", "txt"),
    ("application/json", "json"),
    ("audio/midi", "midi"),
    ("application/x-latex", "latex"),
    ("audio/x-mpegurl", "m3u"),
    ("image/x-macpaint", "pnt"),
    ("text/troff", "tr"),
    ("application/mathml+xml", "mathml"),
    ("application/x-mif", "mif"),
    ("video/quicktime", "qt"),
    ("video/x-sgi-movie", "movie"),
    ("audio/mpeg", "mpa"),
    ("video/mp4", "mp4"),
    ("video/mpeg", "mpg"),
    ("video/mpeg2", "mpv2"),
    ("application/x-wais-source", "src"),
    ("application/x-netcdf", "nc"),
    ("application/oda", "oda"),
    ("application/vnd.oasis.opendocument.database", "odb"),
    ("application/vnd.oasis.opendocument.chart", "odc"),
    ("application/vnd.oasis.opendocument.formula", "odf"),
    ("application/vnd.oasis.opendocument.graphics", "odg"),
    ("application/vnd.oasis.opendocument.image", "odi"),
    ("application/vnd.oasis.opendocument.text-master", "odm"),
    ("application/vnd.oasis.opendocument.presentation", "odp"),
    ("application/vnd.oasis.opendocument.spreadsheet", "ods"),
    ("application/vnd.oasis.opendocument.text", "odt"),
    (
        "application/vnd.oasis.opendocument.graphics-template",
        "otg",
    ),
    ("application/vnd.oasis.opendocument.text-web", "oth"),
    (
        "application/vnd.oasis.opendocument.presentation-template",
        "otp",
    ),
    (
        "application/vnd.oasis.opendocument.spreadsheet-template",
        "ots",
    ),
    ("application/vnd.oasis.opendocument.text-template", "ott"),
    ("application/ogg", "ogx"),
    ("video/ogg", "ogv"),
    ("audio/ogg", "spx"),
    ("application/x-font-opentype", "otf"),
    ("audio/flac", "flac"),
    ("application/annodex", "anx"),
    ("audio/annodex", "axa"),
    ("video/annodex", "axv"),
    ("application/xspf+xml", "xspf"),
    ("image/x-portable-bitmap", "pbm"),
    ("image/pict", "pict"),
    ("application/pdf", "pdf"),
    ("image/x-portable-graymap", "pgm"),
    ("audio/x-scpls", "pls"),
    ("image/png", "png"),
    ("image/x-portable-anymap", "pnm"),
    ("image/x-portable-pixmap", "ppm"),
    ("application/vnd.ms-powerpoint", "pps"),
    ("image/vnd.adobe.photoshop", "psd"),
    ("image/x-quicktime", "qtif"),
    ("image/x-cmu-raster", "ras"),
    ("application/rdf+xml", "rdf"),
    ("image/x-rgb", "rgb"),
    ("application/vnd.rn-realmedia", "rm"),
    ("application/rtf", "rtf"),
    ("text/richtext", "rtx"),
    ("application/font-sfnt", "sfnt"),
    ("application/x-sh", "sh"),
    ("application/x-shar", "shar"),
    ("application/x-stuffit", "sit"),
    ("application/x-sv4cpio", "sv4cpio"),
    ("application/x-sv4crc", "sv4crc"),
    ("image/svg+xml", "svgz"),
    ("application/x-shockwave-flash", "swf"),
    ("application/x-tar", "tar"),
    ("application/x-tcl", "tcl"),
    ("application/x-tex", "tex"),
    ("application/x-texinfo", "texinfo"),
    ("image/tiff", "tiff"),
    ("text/tab-separated-values", "tsv"),
    ("application/x-font-ttf", "ttf"),
    ("application/x-ustar", "ustar"),
    ("application/voicexml+xml", "vxml"),
    ("image/x-xbitmap", "xbm"),
    ("application/xhtml+xml", "xhtml"),
    ("application/vnd.ms-excel", "xls"),
    ("application/xml", "xsl"),
    ("image/x-xpixmap", "xpm"),
    ("application/xslt+xml", "xslt"),
    ("application/vnd.mozilla.xul+xml", "xul"),
    ("image/x-xwindowdump", "xwd"),
    ("application/vnd.visio", "vsd"),
    ("audio/x-wav", "wav"),
    ("image/vnd.wap.wbmp", "wbmp"),
    ("text/vnd.wap.wml", "wml"),
    ("application/vnd.wap.wmlc", "wmlc"),
    ("text/vnd.wap.wmlsc", "wmls"),
    ("application/vnd.wap.wmlscriptc", "wmlscriptc"),
    ("video/x-ms-wmv", "wmv"),
    ("application/font-woff", "woff"),
    ("application/font-woff2", "woff2"),
    ("model/vrml", "wrl"),
    ("application/wspolicy+xml", "wspolicy"),
    ("application/x-compress", "z"),
    ("application/zip", "zip"),
];

/// File extension LINE expects for `mime` (parameters such as `; charset=` are ignored).
pub fn extension(mime: &str) -> Option<&'static str> {
    let mime = mime.split(';').next().unwrap_or_default().trim();
    MIME_TYPES
        .iter()
        .find(|(m, _)| m.eq_ignore_ascii_case(mime))
        .map(|(_, e)| *e)
}

/// Extensions Android's `MimeTypeMap` knows that [`MIME_TYPES`] spells
/// differently or lacks.
const EXTRA_TYPES: &[(&str, &str)] = &[
    ("jpg", "image/jpeg"),
    ("jpeg", "image/jpeg"),
    ("png", "image/png"),
    ("gif", "image/gif"),
    ("webp", "image/webp"),
    ("heic", "image/heic"),
    ("heif", "image/heif"),
    ("bmp", "image/bmp"),
    ("mp4", "video/mp4"),
    ("m4v", "video/mp4"),
    ("mov", "video/quicktime"),
    ("3gp", "video/3gpp"),
    ("webm", "video/webm"),
    ("m4a", "audio/mp4"),
    ("aac", "audio/aac"),
    ("mp3", "audio/mpeg"),
    ("amr", "audio/amr"),
    ("ogg", "audio/ogg"),
    ("wav", "audio/x-wav"),
    ("pdf", "application/pdf"),
    ("txt", "text/plain"),
    ("csv", "text/comma-separated-values"),
    ("zip", "application/zip"),
    (
        "docx",
        "application/vnd.openxmlformats-officedocument.wordprocessingml.document",
    ),
    (
        "xlsx",
        "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet",
    ),
    (
        "pptx",
        "application/vnd.openxmlformats-officedocument.presentationml.presentation",
    ),
];

/// MIME type for a file extension, as Android's `MimeTypeMap` would give
/// it for an OBS upload's `content-type` (`ox3/g.a()`).
pub fn from_extension(ext: &str) -> Option<&'static str> {
    EXTRA_TYPES
        .iter()
        .find(|(e, _)| e.eq_ignore_ascii_case(ext))
        .map(|(_, m)| *m)
        .or_else(|| {
            MIME_TYPES
                .iter()
                .find(|(_, e)| e.eq_ignore_ascii_case(ext))
                .map(|(m, _)| *m)
        })
}

/// `content-type` of an OBS upload named `name`: the MIME type of its
/// extension, or `""` when there is none or it is unknown.
pub fn content_type_for_name(name: &str) -> &'static str {
    name.rsplit_once('.')
        .and_then(|(_, ext)| from_extension(ext))
        .unwrap_or("")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn content_type_from_name() {
        assert_eq!(content_type_for_name("a.JPG"), "image/jpeg");
        assert_eq!(content_type_for_name("clip.mp4"), "video/mp4");
        assert_eq!(content_type_for_name("voice.m4a"), "audio/mp4");
        assert_eq!(content_type_for_name("report.pdf"), "application/pdf");
        assert_eq!(content_type_for_name("tarball.tar"), "application/x-tar");
        assert_eq!(content_type_for_name("hash"), "");
        assert_eq!(content_type_for_name("x.unknownext"), "");
    }

    #[test]
    fn lookup() {
        assert_eq!(extension("image/jpeg"), Some("jpg"));
        assert_eq!(extension("video/mp4"), Some("mp4"));
        assert_eq!(extension("text/plain; charset=utf-8"), Some("txt"));
        assert_eq!(extension("x/unknown"), None);
    }
}
