use crate::types::{Flashblock, Transaction};
use anyhow::Result;
use arrow::array::{
    ArrayRef, BinaryBuilder, Int32Builder, Int64Builder, ListBuilder, StringBuilder, StructBuilder,
};
use arrow::datatypes::{DataType, Field, Schema};
use arrow::record_batch::RecordBatch;
use parquet::arrow::ArrowWriter;
use parquet::file::properties::WriterProperties;
use std::fs::File;
use std::sync::Arc;

pub struct ParquetWriter;

impl ParquetWriter {
    pub fn write_to_file(
        file_path: &str,
        data: Vec<(Flashblock, Vec<Transaction>)>,
    ) -> Result<u64> {
        let schema = Self::create_schema();
        let record_batch = Self::create_record_batch(schema.clone(), data)?;

        let file = File::create(file_path)?;
        let props = WriterProperties::builder()
            .set_compression(parquet::basic::Compression::SNAPPY)
            .build();

        let mut writer = ArrowWriter::try_new(file, schema, Some(props))?;
        writer.write(&record_batch)?;
        writer.close()?;

        Ok(record_batch.num_rows() as u64)
    }

    fn create_schema() -> Arc<Schema> {
        // Transaction struct fields
        let transaction_fields = vec![
            Field::new("tx_data", DataType::Binary, false),
            Field::new("tx_hash", DataType::Utf8, false),
            Field::new("tx_index", DataType::Int32, false),
            Field::new("created_at", DataType::Int64, false), // Unix timestamp
        ];

        let transaction_struct = DataType::Struct(transaction_fields.into());
        let transactions_list =
            DataType::List(Arc::new(Field::new("item", transaction_struct, false)));

        let fields = vec![
            Field::new("id", DataType::Utf8, false), // UUID as string
            Field::new("builder_id", DataType::Utf8, false), // UUID as string
            Field::new("payload_id", DataType::Utf8, false),
            Field::new("flashblock_index", DataType::Int64, false),
            Field::new("block_number", DataType::Int64, false),
            Field::new("received_at", DataType::Int64, false), // Unix timestamp
            Field::new("transactions", transactions_list, false),
        ];

        Arc::new(Schema::new(fields))
    }

    fn create_record_batch(
        schema: Arc<Schema>,
        data: Vec<(Flashblock, Vec<Transaction>)>,
    ) -> Result<RecordBatch> {
        let _num_rows = data.len();

        // Flashblock field builders
        let mut id_builder = StringBuilder::new();
        let mut builder_id_builder = StringBuilder::new();
        let mut payload_id_builder = StringBuilder::new();
        let mut flashblock_index_builder = Int64Builder::new();
        let mut block_number_builder = Int64Builder::new();
        let mut received_at_builder = Int64Builder::new();

        // Transaction list builder - use the same field definition as in schema
        let transaction_struct_fields = vec![
            Field::new("tx_data", DataType::Binary, false),
            Field::new("tx_hash", DataType::Utf8, false),
            Field::new("tx_index", DataType::Int32, false),
            Field::new("created_at", DataType::Int64, false),
        ];

        let transaction_struct = DataType::Struct(transaction_struct_fields.clone().into());
        let list_field = Field::new("item", transaction_struct, false);

        let struct_builder = StructBuilder::new(
            transaction_struct_fields,
            vec![
                Box::new(BinaryBuilder::new()) as Box<dyn arrow::array::ArrayBuilder>,
                Box::new(StringBuilder::new()),
                Box::new(Int32Builder::new()),
                Box::new(Int64Builder::new()),
            ],
        );

        let mut transactions_list_builder = ListBuilder::new(struct_builder).with_field(list_field);

        for (flashblock, transactions) in data {
            // Add flashblock fields
            id_builder.append_value(flashblock.id.to_string());
            builder_id_builder.append_value(flashblock.builder_id.to_string());
            payload_id_builder.append_value(&flashblock.payload_id);
            flashblock_index_builder.append_value(flashblock.flashblock_index);
            block_number_builder.append_value(flashblock.block_number);
            received_at_builder.append_value(flashblock.received_at.timestamp());

            // Add transactions to the list
            for transaction in transactions {
                let struct_builder = transactions_list_builder.values();
                struct_builder.append(true);

                // Add fields one at a time to avoid multiple mutable borrows
                struct_builder
                    .field_builder::<BinaryBuilder>(0)
                    .unwrap()
                    .append_value(&transaction.tx_data);
                struct_builder
                    .field_builder::<StringBuilder>(1)
                    .unwrap()
                    .append_value(&transaction.tx_hash);
                struct_builder
                    .field_builder::<Int32Builder>(2)
                    .unwrap()
                    .append_value(transaction.tx_index);
                struct_builder
                    .field_builder::<Int64Builder>(3)
                    .unwrap()
                    .append_value(transaction.created_at.timestamp());
            }
            transactions_list_builder.append(true);
        }

        let arrays: Vec<ArrayRef> = vec![
            Arc::new(id_builder.finish()),
            Arc::new(builder_id_builder.finish()),
            Arc::new(payload_id_builder.finish()),
            Arc::new(flashblock_index_builder.finish()),
            Arc::new(block_number_builder.finish()),
            Arc::new(received_at_builder.finish()),
            Arc::new(transactions_list_builder.finish()),
        ];

        Ok(RecordBatch::try_new(schema, arrays)?)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;
    use tempfile::NamedTempFile;
    use uuid::Uuid;

    #[test]
    fn test_archive_write() -> Result<()> {
        let temp_file = NamedTempFile::new()?;
        let file_path = temp_file.path().to_str().unwrap();

        let flashblock = Flashblock {
            id: Uuid::new_v4(),
            builder_id: Uuid::new_v4(),
            payload_id: "test_payload".to_string(),
            flashblock_index: 1,
            block_number: 100,
            received_at: Utc::now(),
        };

        let transactions = vec![Transaction {
            id: Uuid::new_v4(),
            flashblock_id: flashblock.id,
            builder_id: flashblock.builder_id,
            payload_id: flashblock.payload_id.clone(),
            flashblock_index: flashblock.flashblock_index,
            block_number: flashblock.block_number,
            tx_data: vec![1, 2, 3, 4],
            tx_hash: "0x1234".to_string(),
            tx_index: 0,
            created_at: Utc::now(),
        }];

        let data = vec![(flashblock, transactions)];
        let row_count = ParquetWriter::write_to_file(file_path, data)?;

        assert_eq!(row_count, 1);
        Ok(())
    }
}
