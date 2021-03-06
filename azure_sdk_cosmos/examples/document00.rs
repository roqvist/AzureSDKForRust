#[macro_use]
extern crate serde_derive;
// Using the prelude module of the Cosmos crate makes easier to use the Rust Azure SDK for Cosmos
// DB.
use azure_sdk_core::prelude::*;
use azure_sdk_cosmos::prelude::*;
use std::borrow::Cow;
use std::error::Error;

#[derive(Serialize, Deserialize, Debug)]
struct MySampleStruct<'a> {
    a_string: Cow<'a, str>,
    a_number: u64,
    a_timestamp: i64,
}

const DATABASE: &str = "azuresdktestdb";
const COLLECTION: &str = "azuresdktc";

// This code will perform these tasks:
// 1. Find an Azure Cosmos DB called *DATABASE*. If it does not exist, create it.
// 2. Find an Azure Cosmos collection called *COLLECTION* in *DATABASE*.
//      If it does not exist, create it.
// 3. Store an entry in collection *COLLECTION* of database *DATABASE*.
// 4. Delete everything.
#[tokio::main]
async fn main() -> Result<(), Box<dyn Error>> {
    // Let's get Cosmos account and master key from env variables.
    // This helps automated testing.
    let master_key =
        std::env::var("COSMOS_MASTER_KEY").expect("Set env variable COSMOS_MASTER_KEY first!");
    let account = std::env::var("COSMOS_ACCOUNT").expect("Set env variable COSMOS_ACCOUNT first!");

    // First, we create an authorization token. There are two types of tokens, master and resource
    // constrained. Please check the Azure documentation for details. You can change tokens
    // at will and it's a good practice to raise your privileges only when needed.
    let authorization_token = AuthorizationToken::new_master(&master_key)?;

    // Next we will create a Cosmos client. You need an authorization_token but you can later
    // change it if needed.
    let client = ClientBuilder::new(account, authorization_token.clone())?;

    // list_databases will give us the databases available in our account. If there is
    // an error (for example, the given key is not valid) you will receive a
    // specific AzureError. In this example we will look for a specific database
    // so we chain a filter operation.
    let db = client
        .list_databases()
        .execute()
        .await?
        .databases
        .into_iter()
        .find(|db| db.id == DATABASE);

    // If the requested database is not found we create it.
    let database = match db {
        Some(db) => db,
        None => {
            client
                .create_database()
                .with_database_name(&DATABASE)
                .execute()
                .await?
                .database
        }
    };
    println!("database == {:?}", database);

    // Now we look for a specific collection. If is not already present
    // we will create it. The collection creation is more complex and
    // has many options (such as indexing and so on).
    let collection = {
        let collections = client
            .with_database(&database.id)
            .list_collections()
            .execute()
            .await?;

        if let Some(collection) = collections
            .collections
            .into_iter()
            .find(|coll| coll.id == COLLECTION)
        {
            collection
        } else {
            let indexes = IncludedPathIndex {
                kind: KeyKind::Hash,
                data_type: DataType::String,
                precision: Some(3),
            };

            let ip = IncludedPath {
                path: "/*".to_owned(),
                indexes: Some(vec![indexes]),
            };

            let ip = IndexingPolicy {
                automatic: true,
                indexing_mode: IndexingMode::Consistent,
                included_paths: vec![ip],
                excluded_paths: vec![],
            };

            // Notice here we specify the expected performance level.
            // Performance levels have price impact. Also, higher
            // performance levels force you to specify an indexing
            // strategy. Consult the documentation for more details.
            // you can also use the predefined performance levels. For example:
            // `Offer::S2`.
            client
                .with_database(&database.id)
                .create_collection()
                .with_collection_name(&COLLECTION)
                .with_offer(Offer::Throughput(400))
                .with_indexing_policy(&ip)
                .with_partition_key(&("/id".into()))
                .execute()
                .await?
                .collection
        }
    };

    println!("collection = {:?}", collection);

    // Now that we have a database and a collection we can insert
    // data in them. Let's create a Document. The only constraint
    // is that we need an id and an arbitrary, Serializable type.
    let doc = Document::new(
        "unique_id100".to_owned(),
        MySampleStruct {
            a_string: Cow::Borrowed("Something here"),
            a_number: 100,
            a_timestamp: chrono::Utc::now().timestamp(),
        },
    );

    // Now we store the struct in Azure Cosmos DB.
    // Notice how easy it is! :)
    // First we construct a "collection" specific client so we
    // do not need to specify it over and over.
    let database_client = client.with_database(&database.id);
    let collection_client = database_client.with_collection(&collection.id);

    // The method create_document will return, upon success,
    // the document attributes.
    let create_document_response = collection_client
        .create_document()
        .with_document(&doc)
        .with_partition_keys(&(&doc.document_attributes.id).into())
        .execute()
        .await?;
    println!(
        "create_document_response == {:#?}",
        create_document_response
    );

    // Now we list all the documents in our collection. It
    // should show we have 1 document.
    println!("Listing documents...");
    let list_documents_response = collection_client
        .list_documents()
        .execute::<MySampleStruct>()
        .await?;
    println!(
        "list_documents_response contains {} documents",
        list_documents_response.documents.len()
    );

    // Now we get the same document by id.
    let get_document_response = collection_client
        .with_document(&doc)
        .get_document()
        .with_partition_keys(&(&doc.document_attributes.id).into())
        .execute::<MySampleStruct>()
        .await?;
    println!("get_document_response == {:#?}", get_document_response);

    // The document can be no longer there so the result is
    // an Option<Document<T>>
    if let Some(document) = get_document_response.document {
        // Now, for the sake of experimentation, we will update (replace) the
        // document created. We do this only if the original document has not been
        // modified in the meantime. This is called optimistic concurrency.
        // In order to do so, we pass to this replace_document call
        // the etag received in the previous get_document. The etag is an opaque value that
        // changes every time the document is updated. If the passed etag is different in
        // CosmosDB it means something else updated the document before us!
        let replace_document_response = collection_client
            .replace_document()
            .with_document(&doc)
            .with_partition_keys(&(&doc.document_attributes.id).into())
            .with_if_match_condition(IfMatchCondition::Match(&document.document_attributes.etag))
            .execute()
            .await?;
        println!(
            "replace_document_response == {:#?}",
            replace_document_response
        );
    }

    // We will perform some cleanup. First we delete the collection...
    client
        .with_database(&DATABASE)
        .with_collection(&COLLECTION)
        .delete_collection()
        .execute()
        .await?;
    println!("collection deleted");

    // And then we delete the database.
    client
        .with_database(&database.id)
        .delete_database()
        .execute()
        .await?;
    println!("database deleted");

    Ok(())
}
