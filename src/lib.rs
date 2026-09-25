#![forbid(unsafe_code)]

//! The SOAP logic technology — a technology of `xmip-core-logic`.
//!
//! An Envelope arrives on HTTP; the first child of its Body names the operation
//! and *is* the arguments, as XML. A result goes back inside an Envelope, a
//! fault inside a Fault. SOAP 1.1 and 1.2 are read alike and answered in the
//! version they came in; a Send Location speaks the version it is built with.
//!
//! What a WSDL adds — the service's name for an operation, the schema of its
//! arguments — arrives when the `wsdl` contract exists; until then the service
//! is the operation element's namespace and the arguments are typed by the
//! `xml-schema` contract, which every SOAP body satisfies structurally.

use contract::ContractId;
use logic::{
    Arrival, Fault, Header, Invocation, Logic, LogicError, OperationName, Outcome, Reply, Request,
};
use stream::Stream;

const NS_11: &str = "http://schemas.xmlsoap.org/soap/envelope/";
const NS_12: &str = "http://www.w3.org/2003/05/soap-envelope";

/// Which SOAP.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Version {
    V11,
    V12,
}

impl Version {
    const fn namespace(self) -> &'static str {
        match self {
            Version::V11 => NS_11,
            Version::V12 => NS_12,
        }
    }

    const fn media_type(self) -> &'static str {
        match self {
            Version::V11 => "text/xml; charset=utf-8",
            Version::V12 => "application/soap+xml; charset=utf-8",
        }
    }

    fn of(namespace: &str) -> Option<Self> {
        match namespace {
            NS_11 => Some(Version::V11),
            NS_12 => Some(Version::V12),
            _ => None,
        }
    }
}

/// The SOAP technology. `version` is what a Send Location speaks and what a
/// reply defaults to when the arrival's version is not known.
pub struct Soap {
    version: Version,
}

impl Soap {
    #[must_use]
    pub const fn new(version: Version) -> Self {
        Self { version }
    }
}

impl Default for Soap {
    fn default() -> Self {
        Self::new(Version::V11)
    }
}

/// The operation element of an Envelope: its name, its namespace, the XML it
/// spans, and the SOAP version the Envelope was written in.
struct Body<'a> {
    name: String,
    namespace: String,
    xml: &'a str,
    version: Version,
    fault: Option<Fault>,
}

fn read_body(text: &str) -> Result<Body<'_>, LogicError> {
    let document = roxmltree::Document::parse(text)
        .map_err(|error| LogicError::new(format!("not well-formed XML: {error}")))?;
    let envelope = document.root_element();
    let version = envelope
        .tag_name()
        .namespace()
        .and_then(Version::of)
        .filter(|_| envelope.tag_name().name() == "Envelope")
        .ok_or_else(|| LogicError::new("the root is not a SOAP Envelope"))?;
    let body = envelope
        .children()
        .find(|n| n.is_element() && n.tag_name().name() == "Body")
        .ok_or_else(|| LogicError::new("the Envelope has no Body"))?;
    let operation = body
        .children()
        .find(roxmltree::Node::is_element)
        .ok_or_else(|| LogicError::new("the Body is empty"))?;
    let fault = (operation.tag_name().name() == "Fault"
        && operation.tag_name().namespace() == Some(version.namespace()))
    .then(|| read_fault(operation, version));
    Ok(Body {
        name: operation.tag_name().name().to_string(),
        namespace: operation.tag_name().namespace().unwrap_or("").to_string(),
        xml: &text[operation.range()],
        version,
        fault,
    })
}

fn read_fault(fault: roxmltree::Node<'_, '_>, version: Version) -> Fault {
    let text_of = |path: &[&str]| -> String {
        let mut node = Some(fault);
        for step in path {
            node = node.and_then(|n| {
                n.children()
                    .find(|c| c.is_element() && c.tag_name().name() == *step)
            });
        }
        node.and_then(|n| n.text()).unwrap_or("").trim().to_string()
    };
    match version {
        Version::V11 => Fault {
            code: text_of(&["faultcode"]),
            message: text_of(&["faultstring"]),
        },
        Version::V12 => Fault {
            code: text_of(&["Code", "Value"]),
            message: text_of(&["Reason", "Text"]),
        },
    }
}

fn envelope(version: Version, inner: &str) -> String {
    format!(
        "<soap:Envelope xmlns:soap=\"{}\"><soap:Body>{inner}</soap:Body></soap:Envelope>",
        version.namespace()
    )
}

fn fault_xml(version: Version, fault: &Fault) -> String {
    match version {
        Version::V11 => format!(
            "<soap:Fault><faultcode>{}</faultcode><faultstring>{}</faultstring></soap:Fault>",
            codec::xml::escape(&fault.code),
            codec::xml::escape(&fault.message)
        ),
        Version::V12 => format!(
            "<soap:Fault><soap:Code><soap:Value>{}</soap:Value></soap:Code>\
             <soap:Reason><soap:Text xml:lang=\"en\">{}</soap:Text></soap:Reason></soap:Fault>",
            codec::xml::escape(&fault.code),
            codec::xml::escape(&fault.message)
        ),
    }
}

fn text_of(stream: &Stream) -> Result<&str, LogicError> {
    std::str::from_utf8(stream.bytes())
        .map_err(|error| LogicError::new(format!("not UTF-8: {error}")))
}

fn version_parameter(parameters: &[Header]) -> Option<Version> {
    parameters
        .iter()
        .find(|p| p.name == "soap-version")
        .and_then(|p| match p.value.as_str() {
            "1.1" => Some(Version::V11),
            "1.2" => Some(Version::V12),
            _ => None,
        })
}

impl Logic for Soap {
    fn technology(&self) -> &'static str {
        "soap"
    }

    fn invocation(&self, arrival: &Arrival<'_>) -> Result<Invocation, LogicError> {
        let text = text_of(arrival.body)?;
        let body = read_body(text)?;
        if body.fault.is_some() {
            return Err(LogicError::new("a Fault is an outcome, not an invocation"));
        }
        let mut parameters = vec![Header::new(
            "soap-version",
            if body.version == Version::V11 {
                "1.1"
            } else {
                "1.2"
            },
        )];
        if let Some(action) = arrival
            .headers
            .iter()
            .find(|h| h.name.eq_ignore_ascii_case("SOAPAction"))
        {
            parameters.push(Header::new("soap-action", action.value.trim_matches('"')));
        }
        Ok(Invocation {
            operation: OperationName::new(body.namespace, body.name),
            arguments: Stream::new(
                arrival.body.id(),
                body.xml.as_bytes().to_vec(),
                Some("application/xml".to_string()),
            ),
            parameters,
            contract: Some(ContractId("xml-schema".to_string())),
        })
    }

    fn reply(&self, invocation: &Invocation, outcome: &Outcome) -> Result<Reply, LogicError> {
        let version = version_parameter(&invocation.parameters).unwrap_or(self.version);
        let (status, inner) = match outcome {
            Outcome::Result(result) => ("200", text_of(result)?.to_string()),
            Outcome::Fault(fault) => ("500", fault_xml(version, fault)),
        };
        Ok(Reply {
            trailers: Vec::new(),
            headers: vec![
                Header::new(":status", status),
                Header::new("Content-Type", version.media_type()),
            ],
            body: Stream::new(
                invocation.arguments.id(),
                envelope(version, &inner).into_bytes(),
                Some(version.media_type().to_string()),
            ),
        })
    }

    fn request(&self, invocation: &Invocation) -> Result<Request, LogicError> {
        let arguments = text_of(&invocation.arguments)?;
        let target = invocation
            .parameters
            .iter()
            .find(|p| p.name == "target")
            .map_or("/", |p| p.value.as_str());
        let action = format!(
            "{}/{}",
            invocation.operation.service, invocation.operation.name
        );
        let mut headers = vec![Header::new("Content-Type", self.version.media_type())];
        if self.version == Version::V11 {
            headers.push(Header::new("SOAPAction", format!("\"{action}\"")));
        }
        Ok(Request {
            target: target.to_string(),
            method: "POST".to_string(),
            headers,
            body: Stream::new(
                invocation.arguments.id(),
                envelope(self.version, arguments).into_bytes(),
                Some(self.version.media_type().to_string()),
            ),
        })
    }

    fn outcome(&self, invocation: &Invocation, reply: &Reply) -> Result<Outcome, LogicError> {
        let body = read_body(text_of(&reply.body)?)?;
        Ok(match body.fault {
            Some(fault) => Outcome::Fault(fault),
            None => Outcome::Result(Stream::new(
                invocation.arguments.id(),
                body.xml.as_bytes().to_vec(),
                Some("application/xml".to_string()),
            )),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use xcore::StreamId;

    fn stream(text: &str) -> Stream {
        Stream::new(
            StreamId::new(1),
            text.as_bytes().to_vec(),
            Some("text/xml".to_string()),
        )
    }

    const ENVELOPE: &str = r#"<soap:Envelope xmlns:soap="http://schemas.xmlsoap.org/soap/envelope/">
  <soap:Body><o:PlaceOrder xmlns:o="urn:example:orders"><o:sku>X</o:sku></o:PlaceOrder></soap:Body>
</soap:Envelope>"#;

    #[test]
    fn an_envelope_becomes_a_named_operation_with_its_xml() {
        let body = stream(ENVELOPE);
        let headers = vec![Header::new(
            "SOAPAction",
            "\"urn:example:orders/PlaceOrder\"",
        )];
        let arrival = Arrival {
            target: "/orders",
            method: "POST",
            headers: &headers,
            body: &body,
        };
        let invocation = Soap::default().invocation(&arrival).expect("invocation");
        assert_eq!(
            invocation.operation,
            OperationName::new("urn:example:orders", "PlaceOrder")
        );
        let arguments = std::str::from_utf8(invocation.arguments.bytes()).expect("xml");
        assert!(arguments.starts_with("<o:PlaceOrder") && arguments.ends_with("</o:PlaceOrder>"));
        assert!(
            invocation
                .parameters
                .iter()
                .any(|p| p.name == "soap-action")
        );
    }

    #[test]
    fn a_result_and_a_fault_go_back_in_the_arrivals_version() {
        let body = stream(ENVELOPE.replace(NS_11, NS_12).as_str());
        let arrival = Arrival {
            target: "/",
            method: "POST",
            headers: &[],
            body: &body,
        };
        let soap = Soap::default();
        let invocation = soap.invocation(&arrival).expect("invocation");
        let result = soap
            .reply(
                &invocation,
                &Outcome::Result(stream("<o:Placed xmlns:o=\"urn:example:orders\"/>")),
            )
            .expect("reply");
        let text = std::str::from_utf8(result.body.bytes()).expect("xml");
        assert!(text.contains(NS_12) && text.contains("<o:Placed"), "{text}");
        let fault = soap
            .reply(
                &invocation,
                &Outcome::Fault(Fault {
                    code: "soap:Receiver".into(),
                    message: "no stock".into(),
                }),
            )
            .expect("reply");
        let text = std::str::from_utf8(fault.body.bytes()).expect("xml");
        assert!(
            text.contains("<soap:Reason>") && text.contains("no stock"),
            "{text}"
        );
        assert!(
            fault
                .headers
                .iter()
                .any(|h| h.name == ":status" && h.value == "500")
        );
    }

    #[test]
    fn a_request_and_its_outcome_round_trip_on_the_send_side() {
        let soap = Soap::new(Version::V11);
        let invocation = Invocation {
            operation: OperationName::new("urn:example:orders", "PlaceOrder"),
            arguments: stream("<o:PlaceOrder xmlns:o=\"urn:example:orders\"/>"),
            parameters: vec![Header::new("target", "/orders")],
            contract: None,
        };
        let request = soap.request(&invocation).expect("request");
        assert_eq!(request.method, "POST");
        assert_eq!(request.target, "/orders");
        assert!(request.headers.iter().any(|h| h.name == "SOAPAction"));
        let answered = Reply {
            trailers: Vec::new(),
            headers: vec![],
            body: stream(&envelope(
                Version::V11,
                "<o:Placed xmlns:o=\"urn:example:orders\">42</o:Placed>",
            )),
        };
        match soap.outcome(&invocation, &answered).expect("outcome") {
            Outcome::Result(result) => assert!(result.bytes().starts_with(b"<o:Placed")),
            Outcome::Fault(fault) => panic!("unexpected fault {fault:?}"),
        }
        let refused = Reply {
            trailers: Vec::new(),
            headers: vec![],
            body: stream(&envelope(
                Version::V11,
                &fault_xml(
                    Version::V11,
                    &Fault {
                        code: "soap:Client".into(),
                        message: "bad sku".into(),
                    },
                ),
            )),
        };
        match soap.outcome(&invocation, &refused).expect("outcome") {
            Outcome::Fault(fault) => assert_eq!(
                fault,
                Fault {
                    code: "soap:Client".into(),
                    message: "bad sku".into()
                }
            ),
            Outcome::Result(_) => panic!("expected a fault"),
        }
    }

    #[test]
    fn what_is_not_an_envelope_is_refused() {
        let body = stream("<order/>");
        let arrival = Arrival {
            target: "/",
            method: "POST",
            headers: &[],
            body: &body,
        };
        assert!(Soap::default().invocation(&arrival).is_err());
    }
}
